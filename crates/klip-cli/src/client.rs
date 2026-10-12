use klip_daemon::config::get_ipc_socket_path;
use klip_proto::ipc::{deserialize_ipc_msg, serialize_ipc_msg, IpcRequest, IpcResponse};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::UnixStream;

pub struct DaemonClient;

impl DaemonClient {
    pub async fn send_request(req: &IpcRequest) -> Result<IpcResponse, String> {
        let sock_path = get_ipc_socket_path();
        if !sock_path.exists() {
            return Err("Klip daemon is not running. Launch it with `klip start`.".to_string());
        }

        #[cfg(unix)]
        {
            let mut stream = UnixStream::connect(&sock_path).await.map_err(|_| {
                "Klip daemon is not running or not responding. Launch it with `klip start`."
                    .to_string()
            })?;

            let serialized = serialize_ipc_msg(req)?;
            let len_bytes = (serialized.len() as u32).to_be_bytes();

            stream
                .write_all(&len_bytes)
                .await
                .map_err(|e| e.to_string())?;
            stream
                .write_all(&serialized)
                .await
                .map_err(|e| e.to_string())?;
            stream.flush().await.map_err(|e| e.to_string())?;

            let mut resp_len_buf = [0u8; 4];
            stream
                .read_exact(&mut resp_len_buf)
                .await
                .map_err(|e| format!("Failed to read response length: {}", e))?;
            let resp_len = u32::from_be_bytes(resp_len_buf) as usize;

            let mut resp_buf = vec![0u8; resp_len];
            stream
                .read_exact(&mut resp_buf)
                .await
                .map_err(|e| format!("Failed to read response data: {}", e))?;

            deserialize_ipc_msg(&resp_buf)
        }
        #[cfg(not(unix))]
        {
            Err("Unix socket not supported on this platform".to_string())
        }
    }
}
