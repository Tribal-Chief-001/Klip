//! Operations built from several control calls, shared by the subcommands and the TUI.
use crate::client::{ClientError, DaemonClient, OLD_DAEMON_REQUEST_CAP};
use klip_proto::ipc::{IpcRequest, IpcResponse, IPC_PROTOCOL_VERSION};

/// How many bytes one request may carry: asked from the daemon (protocol 2), 8 MiB for older daemons.
pub async fn request_cap(client: &DaemonClient) -> usize {
    match client
        .request(
            &IpcRequest::Hello {
                client_protocol: IPC_PROTOCOL_VERSION,
            },
            OLD_DAEMON_REQUEST_CAP,
        )
        .await
    {
        Ok(IpcResponse::Hello(h)) => h.max_request_bytes,
        _ => OLD_DAEMON_REQUEST_CAP,
    }
}

/// Remove every linked device: one `UnpairAll` call, or, for a daemon that does not know it, one
/// `Unpair` per device. Returns `(removed names, (name, reason) failures)`.
pub async fn unpair_all_compat(
    client: &DaemonClient,
) -> Result<(Vec<String>, Vec<(String, String)>), ClientError> {
    if let Ok(IpcResponse::UnpairedAll { removed, failed }) = client
        .request(&IpcRequest::UnpairAll, OLD_DAEMON_REQUEST_CAP)
        .await
    {
        return Ok((removed, failed));
    }
    let peers = match client
        .request(&IpcRequest::GetStatus, OLD_DAEMON_REQUEST_CAP)
        .await?
    {
        IpcResponse::Status(s) => s.peers,
        IpcResponse::Error(e) => return Err(ClientError::Io(e)),
        other => return Err(ClientError::Io(format!("unexpected reply: {other:?}"))),
    };
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for peer in peers {
        let selector = peer.node_id.fingerprint();
        match client
            .request(&IpcRequest::Unpair { selector }, OLD_DAEMON_REQUEST_CAP)
            .await?
        {
            IpcResponse::Unpaired { name } => removed.push(name),
            IpcResponse::Error(e) => failed.push((peer.name, e)),
            other => failed.push((peer.name, format!("unexpected reply: {other:?}"))),
        }
    }
    Ok((removed, failed))
}
