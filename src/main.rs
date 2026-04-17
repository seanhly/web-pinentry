use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::process;

const PATH_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const MAX_ROUTE_HITS: u8 = 2;

fn write_ok(stdout: &mut impl Write) -> io::Result<()> {
    writeln!(stdout, "OK")?;
    stdout.flush()
}

fn write_data(stdout: &mut impl Write, value: &str) -> io::Result<()> {
    let mut encoded = String::with_capacity(value.len());

    for byte in value.bytes() {
        match byte {
            b'\r' => encoded.push_str("%0D"),
            b'\n' => encoded.push_str("%0A"),
            b'%' => encoded.push_str("%25"),
            _ => encoded.push(char::from(byte)),
        }
    }

    writeln!(stdout, "D {encoded}")?;
    stdout.flush()
}

fn write_err(stdout: &mut impl Write, code: u32, message: &str) -> io::Result<()> {
    writeln!(stdout, "ERR {code} {message}")?;
    stdout.flush()
}

fn assuan_unescape(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &value[index + 1..index + 3];
            if let Ok(decoded) = u8::from_str_radix(hex, 16) {
                output.push(decoded);
                index += 3;
                continue;
            }
        }

        output.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&output).into_owned()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = &value[index + 1..index + 3];
                if let Ok(decoded) = u8::from_str_radix(hex, 16) {
                    output.push(decoded);
                    index += 3;
                } else {
                    output.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&output).into_owned()
}

fn html_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());

    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }

    escaped
}

fn parse_form_value(body: &str, key: &str) -> Option<String> {
    for pair in body.split('&') {
        let mut parts = pair.splitn(2, '=');
        let form_key = parts.next()?;
        let form_value = parts.next().unwrap_or_default();

        if percent_decode(form_key) == key {
            return Some(percent_decode(form_value));
        }
    }

    None
}

fn random_path_segment() -> io::Result<String> {
    let mut bytes = [0u8; 6];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;

    Ok(bytes
        .iter()
        .map(|byte| PATH_CHARS[(byte & 63) as usize] as char)
        .collect())
}

fn advertised_url(route_path: &str) -> io::Result<String> {
    Ok(format!("https://gpg.seanhealy.ie{route_path}"))
}

fn send_url_message(url: &str) -> io::Result<()> {
    let status = process::Command::new("matrix-commander-rs")
        .arg("--message")
        .arg(url)
        .status()?;

    if status.success() {
        return Ok(());
    }

    Err(io::Error::other(format!(
        "matrix-commander-rs exited with status {status}"
    )))
}

/**
 * Allow any connections from docker or 127.0.0.1.
 * Docker IPs looks like: 172.17.x.x or 172.18.x.x.
 */
fn is_allowed_client(address: SocketAddr) -> bool {
    matches!(address.ip(), IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST ||
        (ip.octets()[0] == 172 && ip.octets()[1] >= 16 && ip.octets()[1] <= 31))
}

fn send_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

fn render_form(title: &str, description: &str, prompt: &str, url: &str, route_path: &str) -> String {
    let title = html_escape(title);
    let description = html_escape(description);
    let prompt = html_escape(prompt);
    let url = html_escape(url);
    let route_path = html_escape(route_path);

    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title><style>body{{font-family:sans-serif;background:#f6f4ee;color:#1f1f1f;margin:0;min-height:100vh;display:grid;place-items:center}}main{{width:min(28rem,calc(100vw - 2rem));background:#fffdf8;border:1px solid #d9d1c3;border-radius:16px;padding:2rem;box-shadow:0 16px 50px rgba(40,32,20,.12)}}h1{{margin:0 0 1rem;font-size:1.5rem}}p{{margin:0 0 1.25rem;line-height:1.5}}label{{display:block;font-size:.95rem;margin-bottom:.5rem}}input{{width:100%;box-sizing:border-box;padding:.8rem .9rem;border-radius:10px;border:1px solid #b9ae9a;font:inherit}}button{{margin-top:1rem;width:100%;padding:.85rem 1rem;border:0;border-radius:999px;background:#1f5c4d;color:#fff;font:inherit;font-weight:600;cursor:pointer}}small{{display:block;margin-top:1rem;color:#6a6256}}</style></head><body><main><h1>{title}</h1><p>{description}</p><form method=\"post\" action=\"{route_path}\"><label for=\"pin\">{prompt}</label><input id=\"pin\" name=\"pin\" type=\"password\" autocomplete=\"current-password\" autofocus><button type=\"submit\">Submit</button></form><small>Listening at {url}</small></main></body></html>"
    )
}

fn wait_for_password(title: &str, description: &str, prompt: &str) -> io::Result<String> {
    let bind_address = env::var("WEB_PINENTRY_BIND").unwrap_or_else(|_| "0.0.0.0:7563".to_string());
    let listener = TcpListener::bind(&bind_address)?;
    let route_path = format!("/{}", random_path_segment()?);
    let url = advertised_url(&route_path)?;
    let form_page = render_form(title, description, prompt, &url, &route_path);
    let mut route_hits = 0u8;

    send_url_message(&url)?;

    for connection in listener.incoming() {
        let mut stream = connection?;

        if !is_allowed_client(stream.peer_addr()?) {
            send_http_response(
                &mut stream,
                "403 Forbidden",
                "text/plain; charset=utf-8",
                "Forbidden",
            )?;
            continue;
        }

        let mut reader = BufReader::new(stream.try_clone()?);
        let mut request_line = String::new();

        if reader.read_line(&mut request_line)? == 0 {
            continue;
        }

        let request_line = request_line.trim_end_matches(['\r', '\n']);
        let mut request_parts = request_line.split_whitespace();
        let method = request_parts.next().unwrap_or_default();
        let path = request_parts.next().unwrap_or("/");
        let mut content_length = 0usize;

        loop {
            let mut header = String::new();
            if reader.read_line(&mut header)? == 0 {
                break;
            }

            let header = header.trim_end_matches(['\r', '\n']);
            if header.is_empty() {
                break;
            }

            if let Some((name, value)) = header.split_once(':') {
                if name.eq_ignore_ascii_case("Content-Length") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
        }

        if path == route_path {
            route_hits = route_hits.saturating_add(1);

            if route_hits > MAX_ROUTE_HITS {
                send_http_response(
                    &mut stream,
                    "410 Gone",
                    "text/plain; charset=utf-8",
                    "Link expired",
                )?;
                return Ok(String::new());
            }
        }

        if method.eq_ignore_ascii_case("GET") && path == route_path {
            send_http_response(&mut stream, "200 OK", "text/html; charset=utf-8", &form_page)?;
            continue;
        }

        if method.eq_ignore_ascii_case("POST") && path == route_path {
            let mut body = vec![0; content_length];
            reader.read_exact(&mut body)?;
            let body = String::from_utf8_lossy(&body);
            let password = parse_form_value(&body, "pin").unwrap_or_default();
            let response = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Submitted</title><style>body{font-family:sans-serif;background:#f6f4ee;color:#1f1f1f;margin:0;min-height:100vh;display:grid;place-items:center}main{width:min(24rem,calc(100vw - 2rem));background:#fffdf8;border:1px solid #d9d1c3;border-radius:16px;padding:2rem;text-align:center}</style></head><body><main><h1>Password submitted</h1><p>You can close this tab.</p></main></body></html>";
            send_http_response(&mut stream, "200 OK", "text/html; charset=utf-8", response)?;
            return Ok(password);
        }

        send_http_response(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            "Not found",
        )?;
    }

    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "password server closed before a password was submitted",
    ))
}

#[derive(Default)]
struct PromptState {
    title: Option<String>,
    description: Option<String>,
    prompt: Option<String>,
}

fn main() -> io::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut prompt_state = PromptState::default();

    writeln!(stdout, "OK simple rust pinentry ready")?;
    stdout.flush()?;

    for line in stdin.lock().lines() {
        let line = line?;
        let command = line.trim();

        if command.is_empty() {
            continue;
        }

        let mut parts = command.splitn(2, char::is_whitespace);
        let verb = parts.next().unwrap_or_default();
        let argument = parts.next().unwrap_or("").trim();

        if verb.eq_ignore_ascii_case("GETPIN") {
            let title = prompt_state
                .title
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or("Pinentry");
            let description = prompt_state
                .description
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or("Enter the password in the browser window to continue.");
            let prompt = prompt_state
                .prompt
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or("Password");

            match wait_for_password(title, description, prompt) {
                Ok(password) => {
                    write_data(&mut stdout, &password)?;
                    write_ok(&mut stdout)?;
                }
                Err(error) => {
                    eprintln!("web-pinentry error: {error}");
                    write_err(&mut stdout, 83886179, "canceled")?;
                }
            }

            continue;
        }

        if verb.eq_ignore_ascii_case("BYE") {
            write_ok(&mut stdout)?;
            break;
        }

        if verb.eq_ignore_ascii_case("GETINFO") {
            match argument {
                value if value.eq_ignore_ascii_case("pid") => {
                    write_data(&mut stdout, &process::id().to_string())?;
                }
                value if value.eq_ignore_ascii_case("version") => {
                    write_data(&mut stdout, version)?;
                }
                value if value.eq_ignore_ascii_case("flavor") => {
                    write_data(&mut stdout, "browser")?;
                }
                _ => {}
            }

            write_ok(&mut stdout)?;
            continue;
        }

        if verb.eq_ignore_ascii_case("SETDESC") {
            prompt_state.description = Some(assuan_unescape(argument));
            write_ok(&mut stdout)?;
            continue;
        }

        if verb.eq_ignore_ascii_case("SETPROMPT") {
            prompt_state.prompt = Some(assuan_unescape(argument));
            write_ok(&mut stdout)?;
            continue;
        }

        if verb.eq_ignore_ascii_case("SETTITLE") {
            prompt_state.title = Some(assuan_unescape(argument));
            write_ok(&mut stdout)?;
            continue;
        }

        if verb.eq_ignore_ascii_case("RESET") {
            prompt_state = PromptState::default();
            write_ok(&mut stdout)?;
            continue;
        }

        write_ok(&mut stdout)?;
    }

    Ok(())
}
