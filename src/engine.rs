use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// A process row leaves the table after this much time without inbound
/// traffic, and process IDs that stayed silent for this long are dropped from
/// their row.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct ProcessTraffic {
    pub name: String,
    pub executable_path: Option<String>,
    pub pids: Vec<u32>,
    /// Download rate delivered to the process over the latest metrics window.
    pub bits_per_second: f64,
    /// Bytes delivered to the process since NetLadder started.
    pub total_bytes: u64,
    /// Bytes discarded because the process queue was full.
    pub dropped_bytes: u64,
    pub last_seen: Instant,
}

#[derive(Debug, Default)]
pub struct SharedState {
    pub running: bool,
    pub error: Option<String>,
    pub detected_capacity_bits_per_second: Option<u64>,
    pub order: Vec<String>,
    pub limits_bits_per_second: HashMap<String, u64>,
    pub traffic: HashMap<String, ProcessTraffic>,
}

pub type Shared = Arc<Mutex<SharedState>>;

/// Learns usable download capacity from packet arrival windows. Increases are
/// adopted immediately; decreases are slow so a temporarily slow host is not
/// mistaken for a slower internet connection.
#[derive(Debug, Default)]
pub struct CapacityEstimator {
    estimate: Option<f64>,
}

impl CapacityEstimator {
    const MIN_SAMPLE: f64 = 128_000.0;
    const SAMPLE_HEADROOM: f64 = 1.03;
    const DOWNWARD_DECAY: f64 = 0.995;

    pub fn observe(&mut self, sample_bits_per_second: f64) -> Option<u64> {
        if !sample_bits_per_second.is_finite() || sample_bits_per_second < Self::MIN_SAMPLE {
            return self.detected();
        }

        let target = sample_bits_per_second * Self::SAMPLE_HEADROOM;
        self.estimate = Some(match self.estimate {
            None => target,
            Some(current) if target >= current => target,
            Some(current) => (current * Self::DOWNWARD_DECAY).max(target),
        });
        self.detected()
    }

    fn detected(&self) -> Option<u64> {
        self.estimate.map(|estimate| estimate as u64)
    }
}

/// Owns the packet engine threads. Dropping the handle asks the engine to
/// stop and waits for it, so packets still waiting in process queues are
/// reinjected before the WinDivert handle closes.
pub struct EngineHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn start(shared: Shared) -> EngineHandle {
    let stop = Arc::new(AtomicBool::new(false));

    #[cfg(windows)]
    let thread = Some(crate::windows::spawn_engine(shared, stop.clone()));

    #[cfg(not(windows))]
    let thread = {
        shared.lock().unwrap().error = Some("NetLadder only supports Windows.".into());
        None
    };

    EngineHandle { stop, thread }
}

#[cfg(test)]
mod tests {
    use super::CapacityEstimator;

    #[test]
    fn capacity_estimator_rises_fast_and_falls_slowly() {
        let mut estimator = CapacityEstimator::default();
        assert_eq!(estimator.observe(0.0), None);
        assert_eq!(estimator.observe(10_000_000.0), Some(10_300_000));
        assert_eq!(estimator.observe(100_000_000.0), Some(103_000_000));

        let temporary_slow_host = estimator.observe(20_000_000.0).unwrap();
        assert!(temporary_slow_host > 100_000_000);
    }
}
