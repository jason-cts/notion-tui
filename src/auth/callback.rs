use super::AuthError;
use anyhow::Result;
use reqwest::Url;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

pub(super) async fn wait(
    listener: TcpListener,
    path: &str,
    state: &str,
    duration: Duration,
) -> Result<String> {
    tokio::time::timeout(duration, async {
        loop {
            let (mut stream, _) = listener.accept().await?;
            // A stray connection must not monopolize the single-use callback listener.
            match tokio::time::timeout(Duration::from_secs(2), receive(&mut stream, path, state))
                .await
            {
                Ok(Ok(Some(result))) => return result,
                Ok(Ok(None)) | Ok(Err(_)) | Err(_) => continue,
            }
        }
    })
    .await
    .map_err(|_| AuthError::Timeout)?
}

async fn receive(
    stream: &mut TcpStream,
    path: &str,
    state: &str,
) -> Result<Option<Result<String>>> {
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
        let mut byte = [0];
        if stream.read(&mut byte).await? == 0 {
            return Ok(None);
        }
        request.push(byte[0]);
    }
    if !request.ends_with(b"\r\n\r\n") {
        respond(
            stream,
            "431 Request Header Fields Too Large",
            "Request too large.",
        )
        .await?;
        return Ok(None);
    }
    let first = std::str::from_utf8(&request)
        .unwrap_or("")
        .lines()
        .next()
        .unwrap_or("");
    let parts: Vec<_> = first.split_whitespace().collect();
    if parts.len() != 3
        || parts[0] != "GET"
        || !parts[1].starts_with('/')
        || parts[1].starts_with("//")
    {
        respond(stream, "400 Bad Request", "Invalid callback request.").await?;
        return Ok(None);
    }
    let Ok(url) = Url::parse(&format!("http://127.0.0.1{}", parts[1])) else {
        respond(stream, "400 Bad Request", "Invalid callback URL.").await?;
        return Ok(None);
    };
    if url.path() != path {
        respond(stream, "404 Not Found", "Not found.").await?;
        return Ok(None);
    }
    let pairs: Vec<_> = url.query_pairs().collect();
    let values = |name: &str| {
        pairs
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.as_ref())
            .collect::<Vec<_>>()
    };
    if values("state") != [state] {
        respond(
            stream,
            "400 Bad Request",
            "Invalid login state. Return to the terminal and use its login URL.",
        )
        .await?;
        return Ok(None);
    }
    let codes = values("code");
    let errors = values("error");
    if errors.len() == 1 && codes.is_empty() {
        respond(
            stream,
            "200 OK",
            "Authorization cancelled. You can close this tab.",
        )
        .await?;
        return Ok(Some(Err(AuthError::Denied.into())));
    }
    if codes.len() != 1 || codes[0].is_empty() || !errors.is_empty() {
        respond(
            stream,
            "400 Bad Request",
            "Missing or invalid authorization code.",
        )
        .await?;
        return Ok(None);
    }
    let code = codes[0].to_owned();
    // The code is valid even if the browser closes before reading the response.
    let _ = respond(
        stream,
        "200 OK",
        "Authorization received. Return to the terminal to finish login. You can close this tab.",
    )
    .await;
    Ok(Some(Ok(code)))
}

async fn respond(stream: &mut TcpStream, status: &str, message: &str) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{message}",
        message.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}
