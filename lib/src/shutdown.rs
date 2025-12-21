use std::fmt::{Display, Formatter};
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc};

/// This entity is intended to provide a possibility to gracefully shutdown
/// an async operation.
/// The usage is like the following:
///
/// * on the application side:
/// ```text
/// let shutdown = Shutdown::new();
/// ...
/// shutdown.lock().unwrap().submit();
/// shutdown.lock().unwrap().completion().await;
/// ```
///
/// * inside the library:
/// ```text
/// let (mut shutdown_notification, _shutdown_completion) = {
///     let shutdown = self.shutdown.lock().unwrap();
///     (shutdown.notification_handler(), shutdown.completion_guard())
/// };
/// tokio::select! {
///     x = shutdown_notification.wait() => {
///         match x {
///             Ok(_) => Ok(()), // do graceful things
///             Err(e) => Err(...),
///         }
///     }
///     _ = another.await => Ok(()),
/// }
/// ```
pub struct Shutdown {
    /// Sends messages to [`Notification::notify_rx`]
    notify_tx: broadcast::Sender<()>,
    /// Protects [`Shutdown::completion`] from early return
    shutdown_complete_rx: mpsc::Receiver<()>,
    /// Protects [`Shutdown::completion`] from early return
    shutdown_complete_tx: Option<mpsc::Sender<()>>,
}

pub(crate) struct Notification {
    /// Receives messages from [`Shutdown::notify_tx`]
    notify_rx: broadcast::Receiver<()>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum NotificationError {
    Closed,
}

/// Protects [`Shutdown::completion`] from early return
pub(crate) struct CompletionGuard(#[allow(dead_code)] mpsc::Sender<()>);

impl Shutdown {
    pub fn new() -> Arc<Mutex<Self>> {
        let (notify_tx, _) = broadcast::channel(1);
        let (shutdown_complete_tx, shutdown_complete_rx) = mpsc::channel(1);

        Arc::new(Mutex::new(Self {
            notify_tx,
            shutdown_complete_rx,
            shutdown_complete_tx: Some(shutdown_complete_tx),
        }))
    }

    /// Tell the things to do graceful shutdowns. See [`Notification::wait()`].
    pub fn submit(&self) {
        if let Err(e) = self.notify_tx.send(()) {
            debug!("Failed to submit: {}", e);
        }
    }

    /// Wait until all the things commit graceful shutdowns
    pub async fn completion(&mut self) {
        self.shutdown_complete_tx = None;
        // receiver returns `None` after all the senders are dropped
        let _ = self.shutdown_complete_rx.recv().await;
    }

    /// Get a completion handler which is used to notify the application level of
    /// the shutdown completion. The application is notified right after the guard
    /// is dropped.
    /// May return [`None`] in case shutdown has already been committed.
    pub(crate) fn completion_guard(&self) -> Option<CompletionGuard> {
        self.shutdown_complete_tx
            .as_ref()
            .cloned()
            .map(CompletionGuard)
    }

    /// Get a notification handler which is used to initiate graceful shutdowns
    /// in the things from the application level
    pub(crate) fn notification_handler(&self) -> Notification {
        Notification {
            notify_rx: self.notify_tx.subscribe(),
        }
    }
}

impl Notification {
    /// Wait for a notification from the application level to initiate a graceful shutdown
    pub(crate) async fn wait(&mut self) -> Result<(), NotificationError> {
        loop {
            match self.notify_rx.recv().await {
                Ok(_) => break Ok(()),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break Err(NotificationError::Closed),
            }
        }
    }
}

impl Display for NotificationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            NotificationError::Closed => write!(f, "Channel closed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::shutdown::Shutdown;
    use std::time::Duration;

    #[tokio::test]
    async fn test() {
        let shutdown = Shutdown::new();
        let mut notification = shutdown.lock().unwrap().notification_handler();
        tokio::spawn({
            let shutdown = shutdown.clone();
            async move {
                notification.wait().await.unwrap();
                shutdown.lock().unwrap().completion_guard();
            }
        });

        tokio::time::sleep(Duration::from_secs(1)).await;

        assert_eq!(
            Ok(()),
            tokio::time::timeout(
                Duration::from_secs(5),
                shutdown.lock().unwrap().completion()
            )
            .await
        );
    }
}
