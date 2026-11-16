//! Local TCP fixtures test transport behavior; they are not provider integrations.
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn network_public_address_and_url_policy_denies_ssrf_encodings_and_mixed_dns() {
    for value in [
        "0.0.0.0",
        "10.0.0.1",
        "100.100.100.200",
        "127.0.0.1",
        "169.254.169.254",
        "172.31.0.1",
        "192.0.0.8",
        "192.168.1.1",
        "192.0.2.1",
        "192.88.99.1",
        "198.18.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "255.255.255.255",
        "168.63.129.16",
        "::1",
        "::ffff:8.8.8.8",
        "fc00::1",
        "fe80::1",
        "64:ff9b::a00:1",
        "2001::1",
        "2001:db8::1",
        "2002:0808:0808::1",
        "3fff::1",
    ] {
        assert!(!public_ip(value.parse().unwrap()), "{value}");
    }
    for value in [
        "8.8.8.8",
        "1.1.1.1",
        "2001:4860:4860::8888",
        "2606:4700:4700::1111",
    ] {
        assert!(public_ip(value.parse().unwrap()), "{value}");
    }
    for raw in [
        "file:///etc/passwd",
        "http://localhost/",
        "http://localhost.example/",
        "http://127.1/",
        "http://2130706433/",
        "http://0x7f000001/",
        "http://[::ffff:127.0.0.1]/",
        "http://printer.local/",
        "https://example.com:8443/",
        "https://user:secret@example.com/",
        "https://example.com/?secret=x",
        "https://example.com/#token",
        "https://example.com/\n",
    ] {
        assert!(validate_url(raw).is_err(), "{raw}");
    }
    assert!(validate_url("https://example.com/path/to/file.txt").is_ok());
    assert!(validate_addresses(&[
        "8.8.8.8:443".parse().unwrap(),
        "127.0.0.1:443".parse().unwrap()
    ])
    .is_err());
    assert!(validate_addresses(&[]).is_err());
    assert!(validate_addresses(&vec!["8.8.8.8:443".parse().unwrap(); 17]).is_err());
    assert!(validate_addresses(&["1.1.1.1:443".parse().unwrap()]).is_ok());
    let u = validate_url("https://example.com/file").unwrap();
    assert!(redirect(&u, "http://example.com/plain").is_err());
    assert!(redirect(&u, "https://127.0.0.1/").is_err());
    assert!(redirect(&u, "?token=secret").is_err());
    assert_eq!(redirect(&u, "/next").unwrap().path(), "/next");
}

#[test]
fn network_static_html_decodes_entities_skips_scripts_and_limits_utf8() {
    let input=b"<!doctype html><html><title>Page &amp; title</title><style>secret-css</style><script>secret-script <img src='x'></script><h1>Hello</h1><p>world &lt;3 &#x1F980;</p><!--secret-comment--><iframe>secret-frame</iframe><p hidden>hidden source</p></html>";
    let (s, truncated) = text::extract(input, "text/html; charset=UTF-8", 256).unwrap();
    assert!(!truncated);
    assert!(s.contains("Page & title"));
    assert!(s.contains("Hello world <3 🦀"));
    assert!(s.contains("hidden source"));
    assert!(!s.contains("secret"));
    let (s, t) = text::extract("ab🦀later".as_bytes(), "text/html", 3).unwrap();
    assert!(t);
    assert!(s.starts_with("ab\n[bridge: text truncated"));
    assert!(text::extract(&[255], "text/plain", 256).is_err());
    assert!(text::extract(b"plain", "text/plain;charset=windows-1252", 256).is_err());
    assert!(text::extract(b"%PDF", "application/pdf", 256).is_err());
    let (s, t) = text::extract(b"one\ntwo", "text/plain", 256).unwrap();
    assert!(!t);
    assert_eq!(s, "one\ntwo");
    let deep = format!("{}text{}", "<div>".repeat(30000), "</div>".repeat(30000));
    assert_eq!(
        text::extract(deep.as_bytes(), "text/html", 256).unwrap().0,
        "text"
    );
}

async fn server(response: Vec<u8>) -> (Url, SocketAddr, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0 && request.len() < 8192);
            request.extend_from_slice(&buf[..n]);
        }
        socket.write_all(&response).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    (
        Url::parse(&format!("http://{addr}/fixture")).unwrap(),
        addr,
        handle,
    )
}

#[tokio::test]
async fn network_transport_reads_real_bytes_without_redirect_cookie_or_referer() {
    // Lower-level transport only: production URL/DNS policy rejects this loopback fixture.
    let (url,addr,server)=server(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Type: text/plain\r\nSet-Cookie: token=never-use\r\nConnection: close\r\n\r\nhello".to_vec()).await;
    let response = client(&url, &[addr])
        .unwrap()
        .get(url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .unwrap();
    assert_eq!(body(response, 5).await.unwrap(), b"hello");
    let request = server.await.unwrap().to_lowercase();
    assert!(request.contains("accept-encoding: identity"));
    assert!(
        !request.contains("authorization:")
            && !request.contains("cookie:")
            && !request.contains("referer:")
    );
    let (url, addr, server) = server_redirect().await;
    let response = client(&url, &[addr])
        .unwrap()
        .get(url.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 302);
    assert!(redirect(
        &url,
        response.headers()[reqwest::header::LOCATION]
            .to_str()
            .unwrap()
    )
    .is_err());
    server.await.unwrap();
}
async fn server_redirect() -> (Url, SocketAddr, tokio::task::JoinHandle<String>) {
    server(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()).await
}

#[tokio::test]
async fn network_transport_refuses_declared_streamed_and_encoded_oversize() {
    for raw in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n123456".as_slice(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n6\r\n123456\r\n0\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx".as_slice(),
    ]{
        let (url,addr,server)=server(raw.to_vec()).await;
        let response=client(&url,&[addr]).unwrap().get(url).send().await.unwrap();assert!(body(response,5).await.is_err());server.await.unwrap();
    }
}

#[tokio::test]
async fn network_transport_uses_pinned_addresses_without_resolving_hostname_again() {
    let (_, addr, server) =
        server(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\npinned".to_vec())
            .await;
    let url = Url::parse("http://bridge-public-test.invalid/pinned").unwrap();
    let response = client(&url, &[addr])
        .unwrap()
        .get(url)
        .send()
        .await
        .unwrap();
    assert_eq!(response.remote_addr(), Some(addr));
    assert_eq!(body(response, 6).await.unwrap(), b"pinned");
    assert!(server
        .await
        .unwrap()
        .contains("host: bridge-public-test.invalid"));
}
