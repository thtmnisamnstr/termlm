use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use termlm_protocol::{ClientMessage, MAX_FRAME_BYTES, ServerMessage};
use tokio::net::UnixStream;
use tokio_serde::formats::Json;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

pub type ServerTransport = tokio_serde::Framed<
    Framed<UnixStream, LengthDelimitedCodec>,
    ClientMessage,
    ServerMessage,
    Json<ClientMessage, ServerMessage>,
>;

pub fn make_transport(stream: UnixStream) -> ServerTransport {
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME_BYTES)
        .new_codec();
    let framed = Framed::new(stream, codec);
    tokio_serde::Framed::new(framed, Json::<ClientMessage, ServerMessage>::default())
}

pub async fn send_protocol_error(
    transport: &mut ServerTransport,
    message: impl Into<String>,
) -> Result<()> {
    transport
        .send(ServerMessage::Error {
            task_id: None,
            kind: termlm_protocol::ErrorKind::BadProtocol,
            message: message.into(),
            matched_pattern: None,
        })
        .await?;
    Ok(())
}

pub async fn recv_message(
    transport: &mut ServerTransport,
) -> Option<Result<ClientMessage, String>> {
    match transport.next().await {
        Some(Ok(msg)) => Some(Ok(msg)),
        Some(Err(e)) => Some(Err(e.to_string())),
        None => None,
    }
}
