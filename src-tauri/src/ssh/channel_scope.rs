//! Short-lived protocol channels with explicit cancellation/close. russh 0.46
//! ChannelStream has no Drop-based channel close, so dropping a pending read is
//! insufficient. The relay owns the Channel until its Close has been sent.
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio_util::sync::CancellationToken;

pub struct ChannelScope {
    cancel: CancellationToken,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    pending: Mutex<Vec<russh::Channel<russh::client::Msg>>>,
    writers: Mutex<Vec<tokio::fs::File>>,
}
impl ChannelScope {
    pub fn new(parent: &CancellationToken) -> Self {
        Self {
            cancel: parent.child_token(),
            tasks: Mutex::new(Vec::new()),
            pending: Mutex::new(Vec::new()),
            writers: Mutex::new(Vec::new()),
        }
    }

    pub fn stream(&self, mut channel: russh::Channel<russh::client::Msg>) -> DuplexStream {
        let (stream, mut relay) = tokio::io::duplex(64 * 1024);
        let cancel = self.cancel.clone();
        let task = tokio::spawn(async move {
            let forwarding = async {
                let mut buffer = [0u8; 32 * 1024];
                loop {
                    tokio::select! {
                        message = channel.wait() => match message {
                            Some(russh::ChannelMsg::Data { data }) => {
                                if relay.write_all(&data).await.is_err() { break; }
                            }
                            Some(russh::ChannelMsg::Eof | russh::ChannelMsg::Close) | None => break,
                            _ => {}
                        },
                        read = relay.read(&mut buffer) => match read {
                            Ok(0) | Err(_) => break,
                            Ok(count) => if channel.data(&buffer[..count]).await.is_err() { break; },
                        }
                    }
                }
            };
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {},
                _ = forwarding => {},
            }
            // Closing this one channel does not lock/disconnect the shared SSH
            // handle or disturb the terminal/normal transfer channels.
            if tokio::time::timeout(std::time::Duration::from_secs(2), channel.close())
                .await
                .is_err()
            {
                tracing::warn!("Quick Look SSH channel close could not be queued before timeout");
            }
        });
        self.tasks.lock().unwrap().push(task);
        stream
    }

    fn close_channel(&self, channel: russh::Channel<russh::client::Msg>) {
        // Guard::drop may run during runtime teardown: queue the channel here,
        // and perform async work only in finish(), never spawn from Drop.
        self.pending.lock().unwrap().push(channel);
    }

    pub async fn finish(&self) {
        self.cancel.cancel();
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        for channel in pending {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), channel.close()).await;
        }
        // Tokio file writes can still be running in the blocking pool after
        // their owning future is dropped. Flush them before staging is deleted
        // or a replacement request is allowed to acquire the writer gate.
        let writers = std::mem::take(&mut *self.writers.lock().unwrap());
        for mut writer in writers {
            let _ = writer.flush().await;
        }
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
        for task in tasks {
            let _ = task.await;
        }
    }
}
impl Drop for ChannelScope {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Protect the setup phase too: cancellation while waiting for subsystem/exec
/// setup must close the channel even before its byte relay has been installed.
pub struct ChannelGuard<'a> {
    channel: Option<russh::Channel<russh::client::Msg>>,
    scope: Option<&'a ChannelScope>,
}
impl<'a> ChannelGuard<'a> {
    pub fn new(
        channel: russh::Channel<russh::client::Msg>,
        scope: Option<&'a ChannelScope>,
    ) -> Self {
        Self {
            channel: Some(channel),
            scope,
        }
    }
    pub fn into_channel(mut self) -> russh::Channel<russh::client::Msg> {
        self.channel.take().unwrap()
    }
}
impl std::ops::Deref for ChannelGuard<'_> {
    type Target = russh::Channel<russh::client::Msg>;
    fn deref(&self) -> &Self::Target {
        self.channel.as_ref().unwrap()
    }
}
impl std::ops::DerefMut for ChannelGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.channel.as_mut().unwrap()
    }
}
impl Drop for ChannelGuard<'_> {
    fn drop(&mut self) {
        if let (Some(scope), Some(channel)) = (self.scope, self.channel.take()) {
            scope.close_channel(channel);
        }
    }
}

/// Cancellation-safe ownership of local writes belonging to the channel scope.
/// The unscoped case preserves the existing normal-download behavior.
pub struct ScopedFile<'a> {
    file: Option<tokio::fs::File>,
    scope: Option<&'a ChannelScope>,
}
impl<'a> ScopedFile<'a> {
    pub fn new(file: tokio::fs::File, scope: Option<&'a ChannelScope>) -> Self {
        Self {
            file: Some(file),
            scope,
        }
    }
    pub async fn finish(mut self) -> std::io::Result<()> {
        self.file.as_mut().unwrap().flush().await?;
        self.file.take();
        Ok(())
    }
}
impl tokio::io::AsyncWrite for ScopedFile<'_> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(self.get_mut().file.as_mut().unwrap()).poll_write(cx, buffer)
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(self.get_mut().file.as_mut().unwrap()).poll_flush(cx)
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(self.get_mut().file.as_mut().unwrap()).poll_shutdown(cx)
    }
}
impl Drop for ScopedFile<'_> {
    fn drop(&mut self) {
        if let (Some(file), Some(scope)) = (self.file.take(), self.scope) {
            scope.writers.lock().unwrap().push(file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancelled_writer_is_flushed_and_released_before_scope_cleanup_returns() {
        let parent = tempfile::tempdir().unwrap();
        let path = parent.path().join("download.part");
        let cancel = CancellationToken::new();
        let scope = ChannelScope::new(&cancel);
        let file = tokio::fs::File::create(&path).await.unwrap();
        let mut writer = ScopedFile::new(file, Some(&scope));
        writer.write_all(&[42; 32 * 1024]).await.unwrap();
        cancel.cancel();
        drop(writer); // Pending filesystem work belongs to the scope now.
        scope.finish().await;
        assert_eq!(std::fs::read(&path).unwrap(), vec![42; 32 * 1024]);
        assert!(scope.writers.lock().unwrap().is_empty());
        std::fs::remove_file(path).unwrap();
    }
}
