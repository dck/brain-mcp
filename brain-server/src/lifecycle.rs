use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

pub struct ClientTracker {
    count: Arc<AtomicUsize>,
    shutdown_tx: watch::Sender<bool>,
    grace_period: Duration,
}

impl ClientTracker {
    pub fn new(shutdown_tx: watch::Sender<bool>, grace_period: Duration) -> Self {
        Self {
            count: Arc::new(AtomicUsize::new(0)),
            shutdown_tx,
            grace_period,
        }
    }

    pub fn connect(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    pub fn disconnect(&self) {
        let prev = self.count.fetch_sub(1, Ordering::SeqCst);
        if prev == 1 {
            // Last client disconnected — start grace period.
            let tx = self.shutdown_tx.clone();
            let grace = self.grace_period;
            let count = Arc::clone(&self.count);
            tokio::spawn(async move {
                tokio::time::sleep(grace).await;
                if count.load(Ordering::SeqCst) == 0 {
                    let _ = tx.send(true);
                }
            });
        }
    }

    pub fn client_count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_client_tracker_count() {
        let (tx, _rx) = watch::channel(false);
        let tracker = ClientTracker::new(tx, Duration::from_secs(30));

        assert_eq!(tracker.client_count(), 0);

        tracker.connect();
        assert_eq!(tracker.client_count(), 1);

        tracker.connect();
        assert_eq!(tracker.client_count(), 2);

        tracker.disconnect();
        assert_eq!(tracker.client_count(), 1);

        tracker.disconnect();
        assert_eq!(tracker.client_count(), 0);
    }
}
