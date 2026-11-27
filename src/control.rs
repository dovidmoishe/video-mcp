use anyhow::{Result, bail};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Clone)]
pub struct Control {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
}
impl Control {
    pub fn new(timeout: Duration) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now() + timeout,
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn check(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            bail!("operation cancelled");
        }
        if Instant::now() >= self.deadline {
            bail!("operation deadline exceeded");
        }
        Ok(())
    }
}
/// Dropping a request signals its worker; native owners stay alive until it exits.
pub struct CancelOnDrop(pub Control);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_and_deadline() {
        let c = Control::new(Duration::from_secs(1));
        c.check().unwrap();
        drop(CancelOnDrop(c.clone()));
        assert!(c.check().is_err());
        assert!(Control::new(Duration::ZERO).check().is_err());
    }
}
