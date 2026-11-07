//! Stateful loopback native wire fixtures. Simulation only, never vendor certification.
pub mod cql;
pub mod mongo;
pub mod nebula;
pub mod rocket;

use std::sync::{Arc, Mutex};
use tokio::{net::{TcpListener, TcpStream}, task::{JoinHandle, JoinSet}};

pub type Records = Arc<Mutex<Vec<Vec<Option<String>>>>>;
pub fn seed() -> Records { Arc::new(Mutex::new(Vec::new())) }

pub struct Server {
    pub port: u16,
    task: Option<JoinHandle<()>>,
    pub log: Arc<Mutex<Vec<String>>>,
}
impl Server {
    pub fn events(&self) -> Vec<String> { self.log.lock().unwrap().clone() }
    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
        // Rebinding proves the owned listener has actually gone away.
        let listener = TcpListener::bind(("127.0.0.1", self.port)).await?;
        drop(listener);
        Ok(())
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() { task.abort(); }
    }
}
pub async fn serve<F, Fut>(f: F) -> anyhow::Result<Server>
where
    F: Fn(TcpStream, Arc<Mutex<Vec<String>>>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let log = Arc::new(Mutex::new(Vec::new()));
    let events = log.clone();
    let task = tokio::spawn(async move {
        // Aborting this task drops JoinSet, which aborts all active connections.
        let mut jobs = JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        let fut = f(stream, events.clone());
                        let errors = events.clone();
                        jobs.spawn(async move {
                            if let Err(error) = fut.await {
                                errors.lock().unwrap().push(format!("wire_error:{error:#}"));
                            }
                        });
                    }
                    Err(error) => {
                        events.lock().unwrap().push(format!("listener_error:{error}"));
                        break;
                    }
                },
                Some(_) = jobs.join_next(), if !jobs.is_empty() => {}
            }
        }
    });
    Ok(Server { port, task: Some(task), log })
}
pub fn put_string(bytes: &mut Vec<u8>, text: &str) {
    bytes.extend((text.len() as i32).to_be_bytes());
    bytes.extend(text.as_bytes());
}
