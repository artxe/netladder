use std::{
    collections::{HashMap, HashSet, VecDeque},
    mem::size_of,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, bounded};
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
    engine::{CapacityEstimator, ProcessTraffic, Shared},
    process,
};

const UNKNOWN_PROCESS: &str = "기타 / 식별 중";

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
            self.tokens = self.burst_bytes();
            self.last_refill = now;
        }
    }

    fn try_take(&mut self, bytes: usize, now: Instant) -> bool {
        self.refill(now);
        if self.rate_bytes_per_second.is_none() || self.tokens >= bytes as f64 {
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
}

pub fn spawn_engine(shared: Shared, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        if let Err(error) = run_engine(shared.clone(), stop) {
            let mut state = shared.lock().unwrap();
            state.running = false;
            state.error = Some(error);
        }
    });
}

fn run_engine(shared: Shared, stop: Arc<AtomicBool>) -> Result<(), String> {
    let divert = Arc::new(
        WinDivert::network(
            "inbound and !loopback and (tcp or udp)",
            0,
            WinDivertFlags::default(),
        )
        .map_err(|error| format!("패킷 드라이버를 시작하지 못했습니다: {error}"))?,
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
    let mut identities = HashMap::new();
    let mut last_refresh = Instant::now() - Duration::from_secs(1);

    while !stop.load(Ordering::Acquire) {
        if last_refresh.elapsed() >= Duration::from_millis(400) {
            owners = socket_owners()?;
            identities = process::process_identities();
            last_refresh = Instant::now();
        }

        let Some(packet) = divert
            .recv_wait(&mut buffer, 100)
            .map_err(|error| format!("패킷 수신 실패: {error}"))?
        else {
            continue;
        };

        let pid = packet_socket(&packet.data)
            .and_then(|socket| owners.get(&socket).copied())
            .unwrap_or(0);
        let identity = identities.get(&pid);
        let process = identity
            .map(|identity| identity.name.clone())
            .unwrap_or_else(|| UNKNOWN_PROCESS.to_owned());
        let executable_path = identity
            .and_then(|identity| identity.executable_path.as_ref())
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
    let mut queues: HashMap<String, VecDeque<ScheduledPacket>> = HashMap::new();
    let mut interval_bytes: HashMap<String, u64> = HashMap::new();
    let mut total_bytes: HashMap<String, u64> = HashMap::new();
    let mut pids: HashMap<String, HashSet<u32>> = HashMap::new();
    let mut executable_paths: HashMap<String, String> = HashMap::new();
    let mut last_metrics = Instant::now();
    let mut capacity_estimator = CapacityEstimator::default();
    let mut buckets: HashMap<String, TokenBucket> = HashMap::new();
    let mut next_process = 0usize;
    let mut receive_wait = Duration::ZERO;

    while !stop.load(Ordering::Acquire) {
        let queues_empty = queues.values().all(VecDeque::is_empty);
        let received = if queues_empty {
            receiver.recv_timeout(Duration::from_millis(20))
        } else if !receive_wait.is_zero() {
            receiver.recv_timeout(receive_wait)
        } else {
            receiver.try_recv().map_err(|error| match error {
                crossbeam_channel::TryRecvError::Empty => {
                    crossbeam_channel::RecvTimeoutError::Timeout
                }
                crossbeam_channel::TryRecvError::Disconnected => {
                    crossbeam_channel::RecvTimeoutError::Disconnected
                }
            })
        };
        match received {
            Ok(packet) => enqueue(
                packet,
                &mut queues,
                &mut interval_bytes,
                &mut total_bytes,
                &mut pids,
                &mut executable_paths,
            ),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
        }
        for packet in receiver.try_iter().take(4096) {
            enqueue(
                packet,
                &mut queues,
                &mut interval_bytes,
                &mut total_bytes,
                &mut pids,
                &mut executable_paths,
            );
        }
        receive_wait = Duration::ZERO;

        let (order, limits) = {
            let mut state = shared.lock().unwrap();
            for name in queues.keys() {
                if !state.order.contains(name) {
                    state.order.push(name.clone());
                }
            }
            (state.order.clone(), state.limits_bits_per_second.clone())
        };

        let now = Instant::now();
        let mut selected = None;
        let mut shortest_wait = None;
        for offset in 0..order.len() {
            let index = (next_process + offset) % order.len();
            let name = &order[index];
            let Some(packet_bytes) = queues
                .get(name)
                .and_then(VecDeque::front)
                .map(|packet| packet.packet.data.len())
            else {
                continue;
            };
            let bucket = buckets
                .entry(name.clone())
                .or_insert_with(|| TokenBucket::new(now));
            bucket.set_rate(limits.get(name).copied(), now);
            if bucket.try_take(packet_bytes, now) {
                selected = Some((name.clone(), index));
                break;
            }
            let wait = bucket.wait_for(packet_bytes, now);
            shortest_wait = Some(shortest_wait.map_or(wait, |current: Duration| current.min(wait)));
        }

        if let Some((name, index)) = selected {
            let packet = queues
                .get_mut(&name)
                .and_then(VecDeque::pop_front)
                .expect("selected queue must contain a packet");
            if let Err(error) = divert.send(&packet.packet) {
                shared.lock().unwrap().error = Some(format!("패킷 전송 실패: {error}"));
                break;
            }
            next_process = (index + 1) % order.len().max(1);
        } else if let Some(wait) = shortest_wait {
            receive_wait = wait;
        }

        if last_metrics.elapsed() >= Duration::from_millis(500) {
            let elapsed = last_metrics.elapsed();
            let observed_bits_per_second =
                interval_bytes.values().sum::<u64>() as f64 * 8.0 / elapsed.as_secs_f64();
            let detected = capacity_estimator.observe(observed_bits_per_second);
            publish_metrics(
                &shared,
                &mut interval_bytes,
                &total_bytes,
                &pids,
                &executable_paths,
                elapsed,
                detected,
            );
            last_metrics = Instant::now();
        }
    }
}

fn enqueue(
    packet: ScheduledPacket,
    queues: &mut HashMap<String, VecDeque<ScheduledPacket>>,
    interval_bytes: &mut HashMap<String, u64>,
    total_bytes: &mut HashMap<String, u64>,
    pids: &mut HashMap<String, HashSet<u32>>,
    executable_paths: &mut HashMap<String, String>,
) {
    let name = packet.process.clone();
    let bytes = packet.packet.data.len() as u64;
    *interval_bytes.entry(name.clone()).or_default() += bytes;
    *total_bytes.entry(name.clone()).or_default() += bytes;
    if packet.pid != 0 {
        pids.entry(name.clone()).or_default().insert(packet.pid);
    }
    if let Some(path) = &packet.executable_path {
        executable_paths.insert(name.clone(), path.clone());
    }
    queues.entry(name).or_default().push_back(packet);
}

fn publish_metrics(
    shared: &Shared,
    interval_bytes: &mut HashMap<String, u64>,
    total_bytes: &HashMap<String, u64>,
    pids: &HashMap<String, HashSet<u32>>,
    executable_paths: &HashMap<String, String>,
    elapsed: Duration,
    detected_capacity: Option<u64>,
) {
    let now = Instant::now();
    let mut state = shared.lock().unwrap();
    state.detected_capacity_bits_per_second = detected_capacity;
    for (name, bytes) in interval_bytes.drain() {
        let process_total = total_bytes.get(&name).copied().unwrap_or_default();
        let executable_path = executable_paths.get(&name).cloned();
        let mut process_pids: Vec<_> = pids.get(&name).into_iter().flatten().copied().collect();
        process_pids.sort_unstable();
        state.traffic.insert(
            name.clone(),
            ProcessTraffic {
                name,
                executable_path,
                pids: process_pids,
                bits_per_second: bytes as f64 * 8.0 / elapsed.as_secs_f64(),
                total_bytes: process_total,
                last_seen: now,
            },
        );
    }
    for traffic in state.traffic.values_mut() {
        if now.duration_since(traffic.last_seen) > Duration::from_secs(1) {
            traffic.bits_per_second = 0.0;
        }
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
    let mut size = 0u32;
    let first = query(std::ptr::null_mut(), &mut size);
    if first != ERROR_INSUFFICIENT_BUFFER {
        return Err(format!("연결 테이블 크기 조회 실패 (코드 {first})"));
    }
    let mut storage = vec![0u64; (size as usize).div_ceil(size_of::<u64>())];
    let status = query(storage.as_mut_ptr().cast(), &mut size);
    if status != 0 {
        return Err(format!("연결 테이블 조회 실패 (코드 {status})"));
    }

    let base = storage.as_ptr().cast::<u8>();
    let count = unsafe { *base.cast::<u32>() } as usize;
    let rows = unsafe { base.add(size_of::<u32>()).cast::<T>() };
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::TokenBucket;

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
    }
}
