use std::sync::Arc;

use raudo_proto::{AddDownload, Request};
use zbus::fdo;
use zbus::object_server::SignalEmitter;

use crate::manager::Manager;

pub struct Service {
    pub manager: Arc<Manager>,
}

fn failed(e: String) -> fdo::Error {
    fdo::Error::Failed(e)
}

#[zbus::interface(name = "org.raudo.Manager1")]
impl Service {
    fn add(&self, download: &str) -> fdo::Result<i64> {
        let add: AddDownload =
            serde_json::from_str(download).map_err(|e| fdo::Error::InvalidArgs(format!("bad download JSON: {e}")))?;
        self.manager.add(add).map_err(failed)
    }

    fn list(&self) -> fdo::Result<String> {
        let list = self.manager.list().map_err(failed)?;
        Ok(serde_json::to_string(&list).expect("serializable"))
    }

    fn pause(&self, id: i64) -> fdo::Result<()> {
        self.manager.pause(id).map_err(failed)
    }

    fn resume(&self, id: i64) -> fdo::Result<()> {
        self.manager.resume(id).map_err(failed)
    }

    fn remove(&self, id: i64, delete_file: bool) -> fdo::Result<()> {
        self.manager.remove(id, delete_file).map_err(failed)
    }

    fn set_speed_limit(&self, kib_per_sec: u64) {
        self.manager.set_speed_limit(kib_per_sec);
    }

    fn set_max_concurrent(&self, n: u32) -> fdo::Result<()> {
        self.manager.set_max_concurrent(n).map_err(failed)
    }

    async fn inspect(&self, target: &str, request: &str) -> fdo::Result<String> {
        let request: Request =
            serde_json::from_str(request).map_err(|e| fdo::Error::InvalidArgs(format!("bad request JSON: {e}")))?;
        let value = self.manager.inspect(target, request).await.map_err(failed)?;
        Ok(value.to_string())
    }

    async fn formats(&self, request: &str) -> fdo::Result<String> {
        let request: Request =
            serde_json::from_str(request).map_err(|e| fdo::Error::InvalidArgs(format!("bad request JSON: {e}")))?;
        let info = self.manager.formats(&request).await.map_err(failed)?;
        Ok(serde_json::to_string(&info).expect("serializable"))
    }

    #[zbus(signal)]
    pub async fn changed(emitter: &SignalEmitter<'_>, info: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn removed(emitter: &SignalEmitter<'_>, id: i64) -> zbus::Result<()>;
}
