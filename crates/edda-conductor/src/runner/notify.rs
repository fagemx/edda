use std::path::Path;

use edda_notify::NotifyEvent;

/// Notification interface for plan events.
#[async_trait::async_trait]
pub trait Notifier: Send + Sync {
    async fn notify(&self, message: &str);

    /// GH-564: exactly one notification per phase terminal transition
    /// (Passed / Failed / Stale / Skipped / Aborted), carrying plan / phase /
    /// state / attempt and the agent's final output line when available.
    ///
    /// This is the same interface seam #545 defines — the [`Notifier`] trait
    /// in `runner/notify.rs`, with a channel-backed implementation dispatching
    /// through `edda_notify` — not a second mechanism. The default is a no-op
    /// so existing implementations stay unaffected and stdout behavior is
    /// byte-identical ([`StdoutNotifier`] keeps its default).
    async fn notify_phase_terminal(&self, _event: NotifyEvent) {}
}

/// Prints to stdout.
pub struct StdoutNotifier;

#[async_trait::async_trait]
impl Notifier for StdoutNotifier {
    async fn notify(&self, message: &str) {
        println!("[conductor] {message}");
    }
}

/// GH-564/#545 seam: a [`Notifier`] backed by edda-notify channel dispatch.
/// Plain messages fall through to the fallback notifier (stdout stays the
/// always-on channel per #545); phase terminal events go to every configured
/// channel whose `events` list matches `phase_terminal`.
pub struct ChannelNotifier {
    config: edda_notify::NotifyConfig,
    fallback: Box<dyn Notifier>,
}

impl ChannelNotifier {
    pub fn new(config: edda_notify::NotifyConfig, fallback: Box<dyn Notifier>) -> Self {
        Self { config, fallback }
    }

    /// GH-564 P1-1: build the production notifier for `conduct run`.
    /// Loads the channel configuration from `{repo}/.edda/config.json` and
    /// keeps stdout as the always-on fallback. With no channels configured
    /// this behaves exactly like a bare [`StdoutNotifier`]: plain messages
    /// print as before and terminal dispatch is a no-op over zero channels.
    pub fn for_repo(repo_root: &Path) -> Self {
        let paths = edda_ledger::EddaPaths::discover(repo_root);
        Self::new(
            edda_notify::NotifyConfig::load(&paths),
            Box::new(StdoutNotifier),
        )
    }
}

#[async_trait::async_trait]
impl Notifier for ChannelNotifier {
    async fn notify(&self, message: &str) {
        self.fallback.notify(message).await;
    }

    async fn notify_phase_terminal(&self, event: NotifyEvent) {
        let config = self.config.clone();
        // Dispatch is synchronous HTTP (bounded per channel by edda-notify's
        // global timeout); keep it off the async executor threads.
        let _ = tokio::task::spawn_blocking(move || edda_notify::dispatch(&config, &event)).await;
    }
}

/// Collects messages in memory (for testing).
pub struct CollectNotifier {
    messages: std::sync::Mutex<Vec<String>>,
    terminal_events: std::sync::Mutex<Vec<NotifyEvent>>,
}

impl Default for CollectNotifier {
    fn default() -> Self {
        Self::new()
    }
}

impl CollectNotifier {
    pub fn new() -> Self {
        Self {
            messages: std::sync::Mutex::new(Vec::new()),
            terminal_events: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn messages(&self) -> Vec<String> {
        self.messages.lock().unwrap().clone()
    }

    /// GH-564: phase terminal events observed by this notifier.
    pub fn terminal_events(&self) -> Vec<NotifyEvent> {
        self.terminal_events.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Notifier for CollectNotifier {
    async fn notify(&self, message: &str) {
        self.messages.lock().unwrap().push(message.to_string());
    }

    async fn notify_phase_terminal(&self, event: NotifyEvent) {
        self.terminal_events.lock().unwrap().push(event);
    }
}
