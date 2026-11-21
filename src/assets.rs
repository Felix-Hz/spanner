//! UI files compiled into the binary and served from `https://spanner.localhost/`. Nothing is loaded from
//! disk or the network at runtime.

use std::borrow::Cow;

use wry::http::{Response, StatusCode, header};

/// The page may only load what is embedded here and may not connect anywhere.
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; img-src 'self' data:; connect-src 'none'; \
                   base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

macro_rules! assets {
    ($($path:literal => $file:literal, $mime:literal;)*) => {
        const ASSETS: &[(&str, &[u8], &str)] = &[$(($path, include_bytes!(concat!("../ui/", $file)), $mime)),*];
    };
}

assets! {
    "/" => "index.html", "text/html; charset=utf-8";
    "/app.css" => "app.css", "text/css; charset=utf-8";
    "/main.js" => "main.js", "text/javascript; charset=utf-8";
    "/ipc.js" => "ipc.js", "text/javascript; charset=utf-8";
    "/format.js" => "format.js", "text/javascript; charset=utf-8";
    "/configure.js" => "configure.js", "text/javascript; charset=utf-8";
    "/live.js" => "live.js", "text/javascript; charset=utf-8";
    "/history.js" => "history.js", "text/javascript; charset=utf-8";
    "/chart.js" => "chart.js", "text/javascript; charset=utf-8";
    "/shared.js" => "shared.js", "text/javascript; charset=utf-8";
    "/settings.js" => "settings.js", "text/javascript; charset=utf-8";
    "/icons.js" => "icons.js", "text/javascript; charset=utf-8";
    "/icon.svg" => "icon.svg", "image/svg+xml";
    "/fonts/newsreader.woff2" => "fonts/newsreader.woff2", "font/woff2";
    "/fonts/newsreader-display.woff2" => "fonts/newsreader-display.woff2", "font/woff2";
    "/fonts/jetbrains-mono.woff2" => "fonts/jetbrains-mono.woff2", "font/woff2";
    "/fonts/departure-mono.woff2" => "fonts/departure-mono.woff2", "font/woff2";
    "/fonts/instrument-sans.woff2" => "fonts/instrument-sans.woff2", "font/woff2";
}

pub fn respond(path: &str) -> Response<Cow<'static, [u8]>> {
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let path = if path == "/index.html" { "/" } else { path };
    let found = ASSETS.iter().find(|(p, _, _)| *p == path);
    let (status, body, mime): (StatusCode, &'static [u8], &str) = match found {
        Some((_, body, mime)) => (StatusCode::OK, body, mime),
        None => (StatusCode::NOT_FOUND, b"not found", "text/plain"),
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_SECURITY_POLICY, CSP)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Cow::Borrowed(body))
        .expect("static headers are valid")
}

#[cfg(test)]
mod tests {
    #[test]
    fn serves_index_and_rejects_unknown_paths() {
        assert_eq!(super::respond("/").status(), 200);
        assert_eq!(super::respond("/index.html?x=1").status(), 200);
        assert_eq!(super::respond("/../Cargo.toml").status(), 404);
    }
}
