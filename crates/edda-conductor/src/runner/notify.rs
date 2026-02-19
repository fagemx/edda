/// Notification interface for plan events.
#[async_trait::async_trait]
pub trait Notifier: Send + Sync {
    async fn notify(&self, message: &str);
}

/// Prints to stdout.
pub struct StdoutNotifier;

#[async_trait::async_trait]
impl Notifier for StdoutNotifier {
    async fn notify(&self, message: &str) {
        println!("[conductor] {message}");
    }
}

/// Collects messages in memory (for testing).
pub struct CollectNotifier {
    messages: std::sync::Mutex<Vec<String>>,
}

impl CollectNotifier {
    pub fn new() -> Self {
        Self {
            messages: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn messages(&self) -> Vec<String> {
        self.messages.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Notifier for CollectNotifier {
    async fn notify(&self, message: &str) {
        self.messages.lock().unwrap().push(message.to_string());
    }
}
