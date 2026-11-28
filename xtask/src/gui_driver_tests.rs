use super::{Driver, Result};
use reqwest::{Method, blocking::Client};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener},
    time::{Duration, Instant},
};

fn command_response(status: u16, body: String) -> (Result<Value>, String) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "WebDriver fixture was not called"
                    );
                    std::thread::park_timeout(Duration::from_millis(5));
                }
                Err(error) => panic!("WebDriver fixture accept failed: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        loop {
            let mut buffer = [0u8; 1024];
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0, "WebDriver request ended before its body");
            request.extend_from_slice(&buffer[..count]);
            assert!(
                request.len() <= 8192,
                "WebDriver fixture request is oversized"
            );
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..end]).unwrap();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                assert!(length <= 4096, "WebDriver fixture body is oversized");
                if request.len() >= end + 4 + length {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "WebDriver request deadline exceeded"
            );
        }
        let reason = match status {
            200 => "OK",
            400 => "Bad Request",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => panic!("Unexpected fixture status"),
        };
        write!(
            stream,
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        String::from_utf8(request).unwrap()
    });
    let driver = Driver {
        client: Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_millis(500))
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap(),
        base,
        session: "test-session".into(),
    };
    let result = driver.request(
        Method::POST,
        "/execute/async",
        json!({"script":"arguments[arguments.length-1]({accepted:false,error:'application result'});","args":[]}),
    );
    let request = server.join().unwrap();
    assert!(request.starts_with("POST /session/test-session/execute/async HTTP/1.1\r\n"));
    (result, request)
}

#[test]
fn successful_script_values_preserve_application_error_fields() {
    for value in [
        json!({"accepted":false,"error":"Drop request expired; drop the files onto the window again"}),
        json!({"error":"no such element","message":"application data","stacktrace":"literal data"}),
    ] {
        let (result, _) = command_response(200, json!({"value":value.clone()}).to_string());
        assert_eq!(result.unwrap(), value);
    }
}

#[test]
fn webdriver_http_failures_are_rejected_even_with_success_shaped_values() {
    for status in [400, 404, 500] {
        for value in [
            json!({"error":"invalid argument","message":"driver rejected request","stacktrace":""}),
            json!({"accepted":true}),
        ] {
            let (result, _) = command_response(status, json!({"value":value}).to_string());
            let error = result.unwrap_err().to_string();
            assert!(error.contains("route=/execute/async"));
            assert!(error.contains(&status.to_string()));
            assert!(!error.contains("http://"));
        }
    }
}

#[test]
fn malformed_successful_webdriver_json_remains_an_error() {
    let (result, _) = command_response(200, "{invalid JSON".into());
    assert!(result.is_err());
}
