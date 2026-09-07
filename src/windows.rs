use std::{
    collections::{HashMap, VecDeque},
    mem::size_of,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, RecvTimeoutError, TryRecvError, bounded};
use etherparse::{SlicedPacket, TransportSlice};
use windivert::prelude::*;
use windows_sys::Win32::{
    Foundation::ERROR_INSUFFICIENT_BUFFER,
    NetworkManagement::IpHelper::{
        GetExtendedTcpTable, GetExtendedUdpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
        MIB_UDP6ROW_OWNER_PID, MIB_UDPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
    },
    Networking::WinSock::{AF_INET, AF_INET6},
};

use crate::{
    engine::{CapacityEstimator, IDLE_TIMEOUT, ProcessTraffic, Shared},
    process::ProcessDirectory,
};

const UNKNOWN_PROCESS: &str = "Unknown process";
const OWNER_REFRESH_INTERVAL: Duration = Duration::from_millis(400);
const CONFIG_SYNC_INTERVAL: Duration = Duration::from_millis(100);
const METRICS_INTERVAL: Duration = Duration::from_millis(500);
const IDLE_POLL: Duration = Duration::from_millis(20);
const RECEIVE_TIMEOUT_MS: u32 = 100;
/// A limited queue holds roughly this many seconds of traffic at its rate.
/// Packets beyond the cap are dropped so TCP senders slow down instead of the
/// backlog (and latency) growing without bound.
const QUEUE_DELAY_TARGET_SECONDS: f64 = 0.25;
const MIN_QUEUE_BYTES: usize = 128 * 1024;
const MAX_QUEUE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Protocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LocalSocket {
    protocol: Protocol,
    port: u16,
}

struct ScheduledPacket {
    process: String,
    executable_path: Option<String>,
    pid: u32,
    packet: WinDivertPacket<'static, NetworkLayer>,
}

#[derive(Debug)]
struct TokenBucket {
    rate_bytes_per_second: Option<f64>,
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(now: Instant) -> Self {
        Self {
            rate_bytes_per_second: None,
            tokens: 0.0,
            last_refill: now,
        }
    }

    fn set_rate(&mut self, bits_per_second: Option<u64>, now: Instant) {
        self.refill(now);
        let new_rate = bits_per_second.map(|bits| bits as f64 / 8.0);
        if self.rate_bytes_per_second != new_rate {
            self.rate_bytes_per_second = new_rate;
            self.tokens = if new_rate.is_some() {
                self.burst_bytes()
            } else {
                0.0
            };
            self.last_refill = now;
        }
    }

    fn try_take(&mut self, bytes: usize, now: Instant) -> bool {
        self.refill(now);
        if self.rate_bytes_per_second.is_none() {
            return true;
        }
        if self.tokens >= bytes as f64 {
            self.tokens -= bytes as f64;
            true
        } else {
            false
        }
    }

    fn wait_for(&mut self, bytes: usize, now: Instant) -> Duration {
        self.refill(now);
        let Some(rate) = self.rate_bytes_per_second else {
            return Duration::ZERO;
        };
        Duration::from_secs_f64(((bytes as f64 - self.tokens).max(0.0) / rate).min(0.002))
    }

    fn refill(&mut self, now: Instant) {
        if let Some(rate) = self.rate_bytes_per_second {
            self.tokens = (self.tokens + now.duration_since(self.last_refill).as_secs_f64() * rate)
                .min(self.burst_bytes());
        }
        self.last_refill = now;
    }

    fn burst_bytes(&self) -> f64 {
        self.rate_bytes_per_second
            .map(|rate| (rate * 0.010).max(64.0 * 1024.0))
            .unwrap_or(f64::INFINITY)
    }

    fn queue_capacity_bytes(&self) -> usize {
        self.rate_bytes_per_second
            .map(|rate| {
                ((rate * QUEUE_DELAY_TARGET_SECONDS) as usize)
                    .clamp(MIN_QUEUE_BYTES, MAX_QUEUE_BYTES)
            })
            .unwrap_or(MAX_QUEUE_BYTES)
    }
}

/// Everything the scheduler tracks for one process name.
struct ProcessQueue {
    packets: VecDeque<ScheduledPacket>,
    queued_bytes: usize,
    bucket: TokenBucket,
    executable_path: Option<String>,
    pids: HashMap<u32, Instant>,
    interval_bytes: u64,
    total_bytes: u64,
    dropped_bytes: u64,
    last_seen: Instant,
}

impl ProcessQueue {
    fn new(limit_bits_per_second: Option<u64>, now: Instant) -> Self {
        let mut bucket = TokenBucket::new(now);
        bucket.set_rate(limit_bits_per_second, now);
        Self {
            packets: VecDeque::new(),
            queued_bytes: 0,
            bucket,
            executable_path: None,
            pids: HashMap::new(),
            interval_bytes: 0,
            total_bytes: 0,
            dropped_bytes: 0,
            last_seen: now,
        }
    }

    fn enqueue(&mut self, packet: ScheduledPacket, now: Instant) {
        self.last_seen = now;
        if packet.pid != 0 {
            self.pids.insert(packet.pid, now);
        }
        if let Some(path) = &packet.executable_path
            && self.executable_path.as_deref() != Some(path.as_str())
        {
            self.executable_path = Some(path.clone());
        }

        let bytes = packet.packet.data.len();
        if self.queued_bytes + bytes > self.bucket.queue_capacity_bytes() {
            self.dropped_bytes += bytes as u64;
            return;
        }
        self.queued_bytes += bytes;
        self.packets.push_back(packet);
    }

    fn front_len(&self) -> Option<usize> {
        self.packets.front().map(|packet| packet.packet.data.len())
    }

    fn pop_front(&mut self) -> Option<ScheduledPacket> {
        let packet = self.packets.pop_front()?;
        self.queued_bytes -= packet.packet.data.len();
        Some(packet)
    }

    fn record_sent(&mut self, bytes: usize) {
        self.interval_bytes += bytes as u64;
        self.total_bytes += bytes as u64;
    }
}

pub fn spawn_engine(shared: Shared, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    thread::spawn(move || {
        if let Err(error) = run_engine(shared.clone(), stop) {
            let mut state = shared.lock().unwrap();
            state.running = false;
            state.error = Some(error);
        }
    })
}

fn run_engine(shared: Shared, stop: Arc<AtomicBool>) -> Result<(), String> {
    let divert = Arc::new(
        WinDivert::network(
            "inbound and !loopback and (tcp or udp)",
            0,
            WinDivertFlags::default(),
        )
        .map_err(|error| format!("Failed to start the packet driver: {error}"))?,
    );

    {
        let mut state = shared.lock().unwrap();
        state.running = true;
        state.error = None;
    }

    let (sender, receiver) = bounded::<ScheduledPacket>(16_384);
    let scheduler_divert = divert.clone();
    let scheduler_shared = shared.clone();
    let scheduler_stop = stop.clone();
    let scheduler = thread::spawn(move || {
        schedule_packets(scheduler_divert, receiver, scheduler_shared, scheduler_stop)
    });

    let mut buffer = vec![0u8; 65_535];
    let mut owners = HashMap::new();
    let mut directory = ProcessDirectory::new();
    let mut last_refresh = Instant::now() - OWNER_REFRESH_INTERVAL;

    while !stop.load(Ordering::Acquire) {
        if last_refresh.elapsed() >= OWNER_REFRESH_INTERVAL {
            // A transient failure (for example the table growing between the
            // size query and the read) keeps the previous mapping instead of
            // taking the whole engine down.
            if let Ok(fresh) = socket_owners() {
                owners = fresh;
            }
            directory.refresh();
            last_refresh = Instant::now();
        }

        let Some(packet) = divert
            .recv_wait(&mut buffer, RECEIVE_TIMEOUT_MS)
            .map_err(|error| format!("Failed to receive a packet: {error}"))?
        else {
            continue;
        };

        let pid = packet_socket(&packet.data)
            .and_then(|socket| owners.get(&socket).copied())
            .unwrap_or(0);
        let identity = directory.lookup(pid);
        let process = identity
            .map(|identity| identity.name.to_string_lossy().into_owned())
            .unwrap_or_else(|| UNKNOWN_PROCESS.to_owned());
        let executable_path = identity
            .and_then(|identity| identity.executable_path)
            .map(|path| path.display().to_string());

        if sender
            .send(ScheduledPacket {
                process,
                executable_path,
                pid,
                packet: packet.into_owned(),
            })
            .is_err()
        {
            break;
        }
    }

    drop(sender);
    let _ = scheduler.join();
    shared.lock().unwrap().running = false;
    Ok(())
}

fn schedule_packets(
    divert: Arc<WinDivert<NetworkLayer>>,
    receiver: Receiver<ScheduledPacket>,
    shared: Shared,
    stop: Arc<AtomicBool>,
) {
    let mut queues: HashMap<String, ProcessQueue> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut limits: HashMap<String, u64> = HashMap::new();
    let mut last_sync = Instant::now() - CONFIG_SYNC_INTERVAL;
    let mut last_metrics = Instant::now();
    let mut capacity_estimator = CapacityEstimator::default();
    let mut arrived_interval_bytes = 0u64;
    let mut next_process = 0usize;
    let mut receive_wait = Duration::ZERO;
    let mut new_process_seen = false;

    while !stop.load(Ordering::Acquire) {
        let queues_empty = queues.values().all(|queue| queue.packets.is_empty());
        let received = if queues_empty {
            receiver.recv_timeout(IDLE_POLL)
        } else if !receive_wait.is_zero() {
            receiver.recv_timeout(receive_wait)
        } else {
            receiver.try_recv().map_err(|error| match error {
                TryRecvError::Empty => RecvTimeoutError::Timeout,
                TryRecvError::Disconnected => RecvTimeoutError::Disconnected,
            })
        };
        let now = Instant::now();
        let mut enqueue = |packet: ScheduledPacket| {
            arrived_interval_bytes += packet.packet.data.len() as u64;
            let queue = queues.entry(packet.process.clone()).or_insert_with(|| {
                new_process_seen = true;
                ProcessQueue::new(limits.get(&packet.process).copied(), now)
            });
            queue.enqueue(packet, now);
        };
        match received {
            Ok(packet) => enqueue(packet),
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        for packet in receiver.try_iter().take(4096) {
            enqueue(packet);
        }
        receive_wait = Duration::ZERO;

        if new_process_seen || now.duration_since(last_sync) >= CONFIG_SYNC_INTERVAL {
            sync_configuration(&shared, &queues, &mut order, &mut limits);
            for (name, queue) in &mut queues {
                queue.bucket.set_rate(limits.get(name).copied(), now);
            }
            new_process_seen = false;
            last_sync = now;
        }

        let mut selected = None;
        let mut shortest_wait: Option<Duration> = None;
        for offset in 0..order.len() {
            let index = (next_process + offset) % order.len();
            let Some(queue) = queues.get_mut(&order[index]) else {
                continue;
            };
            let Some(packet_bytes) = queue.front_len() else {
                continue;
            };
            if queue.bucket.try_take(packet_bytes, now) {
                selected = Some(index);
                break;
            }
            let wait = queue.bucket.wait_for(packet_bytes, now);
            shortest_wait = Some(shortest_wait.map_or(wait, |current| current.min(wait)));
        }

        if let Some(index) = selected {
            let queue = queues
                .get_mut(&order[index])
                .expect("selected queue must exist");
            let packet = queue
                .pop_front()
                .expect("selected queue must contain a packet");
            if let Err(error) = divert.send(&packet.packet) {
                shared.lock().unwrap().error =
                    Some(format!("Failed to reinject a packet: {error}"));
                break;
            }
            queue.record_sent(packet.packet.data.len());
            next_process = (index + 1) % order.len().max(1);
        } else if let Some(wait) = shortest_wait {
            receive_wait = wait;
        }

        if now.duration_since(last_metrics) >= METRICS_INTERVAL {
            let elapsed = now.duration_since(last_metrics);
            let observed_bits_per_second =
                arrived_interval_bytes as f64 * 8.0 / elapsed.as_secs_f64();
            arrived_interval_bytes = 0;
            let detected = capacity_estimator.observe(observed_bits_per_second);
            publish_metrics(&shared, &mut queues, elapsed, detected, now);
            last_metrics = now;
        }
    }

    drain_channel(&receiver, &mut queues, &limits);
    flush_queues(&divert, &mut queues);
}

/// Moves packets that were still waiting in the channel into their queues so
/// the shutdown flush reinjects them too.
fn drain_channel(
    receiver: &Receiver<ScheduledPacket>,
    queues: &mut HashMap<String, ProcessQueue>,
    limits: &HashMap<String, u64>,
) {
    let now = Instant::now();
    for packet in receiver.try_iter() {
        queues
            .entry(packet.process.clone())
            .or_insert_with(|| ProcessQueue::new(limits.get(&packet.process).copied(), now))
            .enqueue(packet, now);
    }
}

/// Publishes newly discovered process names and pulls the latest limits.
fn sync_configuration(
    shared: &Shared,
    queues: &HashMap<String, ProcessQueue>,
    order: &mut Vec<String>,
    limits: &mut HashMap<String, u64>,
) {
    let mut state = shared.lock().unwrap();
    for name in queues.keys() {
        if !state.order.contains(name) {
            state.order.push(name.clone());
        }
    }
    order.clone_from(&state.order);
    limits.clone_from(&state.limits_bits_per_second);
}

/// Reinjects everything still queued so shutting down does not stall the
/// connections that were being shaped.
fn flush_queues(divert: &WinDivert<NetworkLayer>, queues: &mut HashMap<String, ProcessQueue>) {
    for queue in queues.values_mut() {
        while let Some(packet) = queue.pop_front() {
            if divert.send(&packet.packet).is_err() {
                return;
            }
        }
    }
}

fn publish_metrics(
    shared: &Shared,
    queues: &mut HashMap<String, ProcessQueue>,
    elapsed: Duration,
    detected_capacity: Option<u64>,
    now: Instant,
) {
    let mut state = shared.lock().unwrap();
    state.detected_capacity_bits_per_second = detected_capacity;
    for (name, queue) in queues.iter_mut() {
        queue
            .pids
            .retain(|_, seen| now.duration_since(*seen) <= IDLE_TIMEOUT);
        let mut pids: Vec<u32> = queue.pids.keys().copied().collect();
        pids.sort_unstable();
        let bits_per_second = queue.interval_bytes as f64 * 8.0 / elapsed.as_secs_f64();
        queue.interval_bytes = 0;
        state.traffic.insert(
            name.clone(),
            ProcessTraffic {
                name: name.clone(),
                executable_path: queue.executable_path.clone(),
                pids,
                bits_per_second,
                total_bytes: queue.total_bytes,
                dropped_bytes: queue.dropped_bytes,
                last_seen: queue.last_seen,
            },
        );
    }
}

fn packet_socket(packet: &[u8]) -> Option<LocalSocket> {
    let sliced = SlicedPacket::from_ip(packet).ok()?;
    match sliced.transport? {
        TransportSlice::Tcp(tcp) => Some(LocalSocket {
            protocol: Protocol::Tcp,
            port: tcp.destination_port(),
        }),
        TransportSlice::Udp(udp) => Some(LocalSocket {
            protocol: Protocol::Udp,
            port: udp.destination_port(),
        }),
        _ => None,
    }
}

fn socket_owners() -> Result<HashMap<LocalSocket, u32>, String> {
    let mut result = HashMap::new();
    unsafe {
        read_table::<MIB_TCPROW_OWNER_PID>(
            |buffer, size| {
                GetExtendedTcpTable(buffer, size, 0, AF_INET as u32, TCP_TABLE_OWNER_PID_ALL, 0)
            },
            |row| (row.dwLocalPort, row.dwOwningPid),
            Protocol::Tcp,
            &mut result,
        )?;
        read_table::<MIB_TCP6ROW_OWNER_PID>(
            |buffer, size| {
                GetExtendedTcpTable(buffer, size, 0, AF_INET6 as u32, TCP_TABLE_OWNER_PID_ALL, 0)
            },
            |row| (row.dwLocalPort, row.dwOwningPid),
            Protocol::Tcp,
            &mut result,
        )?;
        read_table::<MIB_UDPROW_OWNER_PID>(
            |buffer, size| {
                GetExtendedUdpTable(buffer, size, 0, AF_INET as u32, UDP_TABLE_OWNER_PID, 0)
            },
            |row| (row.dwLocalPort, row.dwOwningPid),
            Protocol::Udp,
            &mut result,
        )?;
        read_table::<MIB_UDP6ROW_OWNER_PID>(
            |buffer, size| {
                GetExtendedUdpTable(buffer, size, 0, AF_INET6 as u32, UDP_TABLE_OWNER_PID, 0)
            },
            |row| (row.dwLocalPort, row.dwOwningPid),
            Protocol::Udp,
            &mut result,
        )?;
    }
    Ok(result)
}

unsafe fn read_table<T: Copy>(
    query: impl Fn(*mut core::ffi::c_void, *mut u32) -> u32,
    fields: impl Fn(&T) -> (u32, u32),
    protocol: Protocol,
    output: &mut HashMap<LocalSocket, u32>,
) -> Result<(), String> {
    const HEADER_BYTES: usize = size_of::<u32>();
    const ATTEMPTS: usize = 4;

    let mut required = 0u32;
    let first = query(std::ptr::null_mut(), &mut required);
    if first != ERROR_INSUFFICIENT_BUFFER {
        return Err(format!(
            "Failed to size the connection table (code {first})"
        ));
    }

    for _ in 0..ATTEMPTS {
        // Leave headroom so a table that grows between the two calls still fits.
        let capacity = required as usize + required as usize / 4 + 4096;
        let mut storage = vec![0u64; capacity.div_ceil(size_of::<u64>())];
        let mut size = capacity as u32;
        let status = query(storage.as_mut_ptr().cast(), &mut size);
        if status == ERROR_INSUFFICIENT_BUFFER {
            required = size;
            continue;
        }
        if status != 0 {
            return Err(format!(
                "Failed to read the connection table (code {status})"
            ));
        }

        let base = storage.as_ptr().cast::<u8>();
        let count = unsafe { *base.cast::<u32>() } as usize;
        let count = count.min((capacity - HEADER_BYTES) / size_of::<T>());
        let rows = unsafe { base.add(HEADER_BYTES).cast::<T>() };
        for index in 0..count {
            let row = unsafe { &*rows.add(index) };
            let (raw_port, pid) = fields(row);
            output.insert(
                LocalSocket {
                    protocol,
                    port: u16::from_be(raw_port as u16),
                },
                pid,
            );
        }
        return Ok(());
    }
    Err("The connection table kept growing while it was being read".to_owned())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use windivert::prelude::*;

    use std::collections::HashMap;

    use super::{
        MAX_QUEUE_BYTES, MIN_QUEUE_BYTES, ProcessQueue, QUEUE_DELAY_TARGET_SECONDS,
        ScheduledPacket, TokenBucket, drain_channel,
    };

    fn packet(bytes: usize, pid: u32) -> ScheduledPacket {
        ScheduledPacket {
            process: "test.exe".to_owned(),
            executable_path: None,
            pid,
            // The address stays zeroed; these packets are never reinjected.
            packet: unsafe { WinDivertPacket::<NetworkLayer>::new(vec![0u8; bytes]) },
        }
    }

    #[test]
    fn limited_bucket_refills_at_its_own_rate() {
        let now = Instant::now();
        let mut bucket = TokenBucket::new(now);
        bucket.set_rate(Some(8_000), now);

        assert!(bucket.try_take(64 * 1024, now));
        assert!(!bucket.try_take(1_000, now));
        assert!(bucket.try_take(1_000, now + Duration::from_secs(1)));
    }

    #[test]
    fn unlimited_bucket_never_waits() {
        let now = Instant::now();
        let mut bucket = TokenBucket::new(now);
        assert!(bucket.try_take(usize::MAX / 2, now));
        assert_eq!(bucket.wait_for(usize::MAX / 2, now), Duration::ZERO);
        assert!(bucket.try_take(usize::MAX / 2, now));
    }

    #[test]
    fn queue_capacity_scales_with_rate_within_bounds() {
        let now = Instant::now();
        let mut bucket = TokenBucket::new(now);
        assert_eq!(bucket.queue_capacity_bytes(), MAX_QUEUE_BYTES);

        bucket.set_rate(Some(1_000_000), now);
        assert_eq!(bucket.queue_capacity_bytes(), MIN_QUEUE_BYTES);

        bucket.set_rate(Some(100_000_000), now);
        let expected = (100_000_000.0 / 8.0 * QUEUE_DELAY_TARGET_SECONDS) as usize;
        assert_eq!(bucket.queue_capacity_bytes(), expected);

        bucket.set_rate(Some(10_000_000_000), now);
        assert_eq!(bucket.queue_capacity_bytes(), MAX_QUEUE_BYTES);
    }

    #[test]
    fn full_queue_drops_new_packets_and_counts_them() {
        let now = Instant::now();
        let mut queue = ProcessQueue::new(Some(1_000_000), now);
        let packet_bytes = 1_500;
        let fits = MIN_QUEUE_BYTES / packet_bytes;
        for _ in 0..fits {
            queue.enqueue(packet(packet_bytes, 1), now);
        }
        assert_eq!(queue.packets.len(), fits);
        assert_eq!(queue.dropped_bytes, 0);

        queue.enqueue(packet(packet_bytes, 1), now);
        assert_eq!(queue.packets.len(), fits);
        assert_eq!(queue.dropped_bytes, packet_bytes as u64);

        let sent = queue.pop_front().unwrap();
        queue.record_sent(sent.packet.data.len());
        assert_eq!(queue.total_bytes, packet_bytes as u64);
        queue.enqueue(packet(packet_bytes, 1), now);
        assert_eq!(queue.packets.len(), fits);
    }

    #[test]
    fn shutdown_drain_moves_channel_packets_into_queues() {
        let (sender, receiver) = crossbeam_channel::bounded(8);
        sender.send(packet(100, 1)).unwrap();
        sender.send(packet(200, 1)).unwrap();
        let mut queues = HashMap::new();
        let limits = HashMap::from([("test.exe".to_owned(), 1_000_000)]);

        drain_channel(&receiver, &mut queues, &limits);

        let queue = &queues["test.exe"];
        assert_eq!(queue.packets.len(), 2);
        assert_eq!(queue.queued_bytes, 300);
        assert!(queue.bucket.rate_bytes_per_second.is_some());
        assert!(receiver.is_empty());
    }

    #[test]
    fn queue_tracks_pids_by_last_activity() {
        let now = Instant::now();
        let mut queue = ProcessQueue::new(None, now);
        queue.enqueue(packet(100, 10), now);
        queue.enqueue(packet(100, 0), now);
        queue.enqueue(packet(100, 20), now + Duration::from_secs(5));
        assert_eq!(queue.pids.len(), 2);
        assert_eq!(queue.pids[&10], now);
        assert_eq!(queue.pids[&20], now + Duration::from_secs(5));
        assert_eq!(queue.last_seen, now + Duration::from_secs(5));
    }
}
