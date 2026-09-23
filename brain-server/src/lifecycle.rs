use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::watch;

pub const LEASE: Duration = Duration::from_secs(45);
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
pub const REAP_INTERVAL: Duration = Duration::from_secs(1);
pub const SESSION_HEADER: &str = "x-brain-session";

pub struct SessionTracker {
    state: Mutex<Sessions>,
    lease: Duration,
    grace: Option<Duration>,
    shutdown: watch::Sender<bool>,
}

struct Sessions {
    last_seen: HashMap<String, Instant>,
    idle_since: Option<Instant>,
}

impl SessionTracker {
    pub fn new(lease: Duration, grace: Duration, shutdown: watch::Sender<bool>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(Sessions {
                last_seen: HashMap::new(),
                idle_since: None,
            }),
            lease,
            grace: (!grace.is_zero()).then_some(grace),
            shutdown,
        })
    }

    pub fn touch(&self, id: &str) {
        self.touch_at(id, Instant::now());
    }

    pub fn touch_at(&self, id: &str, now: Instant) {
        let mut s = self.state.lock().unwrap();
        if s.last_seen.insert(id.to_string(), now).is_none() {
            tracing::info!(session = id, "session opened");
        }
        s.idle_since = None;
    }

    pub fn close(&self, id: &str) {
        self.close_at(id, Instant::now());
    }

    pub fn close_at(&self, id: &str, now: Instant) {
        let mut s = self.state.lock().unwrap();
        if s.last_seen.remove(id).is_some() {
            tracing::info!(session = id, "session closed");
            if s.last_seen.is_empty() {
                s.idle_since = Some(now);
            }
        }
    }

    pub fn active(&self) -> usize {
        self.state.lock().unwrap().last_seen.len()
    }

    pub fn reap_at(&self, now: Instant) -> bool {
        let mut s = self.state.lock().unwrap();
        let lease = self.lease;
        let before = s.last_seen.len();
        s.last_seen.retain(|id, seen| {
            let alive = now.saturating_duration_since(*seen) <= lease;
            if !alive {
                tracing::info!(session = id.as_str(), "session lease expired");
            }
            alive
        });
        if before > 0 && s.last_seen.is_empty() {
            s.idle_since = Some(now);
        }
        match (self.grace, s.idle_since) {
            (Some(grace), Some(since)) => now.saturating_duration_since(since) >= grace,
            _ => false,
        }
    }

    pub fn spawn_reaper(self: &Arc<Self>, tick: Duration) -> tokio::task::JoinHandle<()> {
        let tracker = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tick).await;
                if tracker.reap_at(Instant::now()) {
                    tracing::info!(
                        grace_secs = tracker.grace.map(|g| g.as_secs()),
                        "no sessions for grace period, shutting down"
                    );
                    let _ = tracker.shutdown.send(true);
                    break;
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker(grace: Duration) -> Arc<SessionTracker> {
        let (tx, _rx) = watch::channel(false);
        SessionTracker::new(LEASE, grace, tx)
    }

    #[test]
    fn touch_counts_sessions() {
        let t = tracker(Duration::ZERO);
        t.touch("a");
        t.touch("b");
        t.touch("a");
        assert_eq!(t.active(), 2);
    }

    #[test]
    fn close_removes_session() {
        let t = tracker(Duration::ZERO);
        t.touch("a");
        t.close("a");
        assert_eq!(t.active(), 0);
        t.close("unknown");
        assert_eq!(t.active(), 0);
    }

    #[test]
    fn lease_expiry_removes_session() {
        let t = tracker(Duration::ZERO);
        let t0 = Instant::now();
        t.touch_at("a", t0);
        assert!(!t.reap_at(t0 + LEASE - Duration::from_secs(1)));
        assert_eq!(t.active(), 1);
        assert!(!t.reap_at(t0 + LEASE + Duration::from_secs(1)));
        assert_eq!(t.active(), 0);
    }

    #[test]
    fn idle_shutdown_after_grace() {
        let t = tracker(Duration::from_secs(60));
        let t0 = Instant::now();
        t.touch_at("a", t0);
        t.close_at("a", t0);
        assert!(!t.reap_at(t0 + Duration::from_secs(59)));
        assert!(t.reap_at(t0 + Duration::from_secs(60)));
    }

    #[test]
    fn reconnect_cancels_idle() {
        let t = tracker(Duration::from_secs(60));
        let t0 = Instant::now();
        t.close_at("a", t0);
        t.touch_at("b", t0 + Duration::from_secs(30));
        assert!(!t.reap_at(t0 + Duration::from_secs(61)));
    }

    #[test]
    fn never_armed_without_sessions() {
        let t = tracker(Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(!t.reap_at(t0 + Duration::from_secs(3600)));
    }

    #[test]
    fn grace_zero_disables() {
        let t = tracker(Duration::ZERO);
        let t0 = Instant::now();
        t.touch_at("a", t0);
        t.close_at("a", t0);
        assert!(!t.reap_at(t0 + Duration::from_secs(3600)));
    }

    #[tokio::test]
    async fn reaper_sends_shutdown() {
        let (tx, mut rx) = watch::channel(false);
        let t = SessionTracker::new(Duration::from_millis(50), Duration::from_millis(50), tx);
        t.touch("a");
        t.spawn_reaper(Duration::from_millis(10));
        tokio::time::timeout(Duration::from_secs(2), rx.wait_for(|v| *v))
            .await
            .expect("shutdown signalled")
            .unwrap();
    }
}
