use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),

    #[error("ws: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("okx: code={code} msg={msg}")]
    Okx { code: String, msg: String },

    #[error("{0}")]
    Msg(String),
}

pub type Result<T> = std::result::Result<T, Error>;
