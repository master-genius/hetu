//! 统一错误类型：所有 command 返回 Result<T, Error>，前端拿到可读的中文错误串。

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("SSH 协议错误: {0}")]
    Ssh(#[from] russh::Error),
    #[error("SSH 密钥错误: {0}")]
    Keys(#[from] russh::keys::Error),
    #[error("SFTP 错误: {0}")]
    Sftp(#[from] russh_sftp::client::error::Error),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Msg(String),
}

impl Error {
    pub fn msg(s: impl Into<String>) -> Self {
        Error::Msg(s.into())
    }
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
