use rewind_config::ShareHost;
use serde_json::Value;

use crate::platform::http::{BROWSER_AGENT, Session};

const BOUNDARY: &str = "----rewind-share-boundary-7f3a9c";
const SUCCESS: std::ops::RangeInclusive<u32> = 200..=299;
const AGENT: &str = "Rewind";
const PIXHOST_THUMB: &str = "420";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uploaded {
    pub link: String,
    pub delete: Option<String>,
}

pub const fn retention(host: ShareHost) -> Option<&'static str> {
    match host {
        ShareHost::Catbox => Some("Хранится бессрочно, файлы до 200 МБ."),
        ShareHost::ImgBb | ShareHost::Pixhost => None,
    }
}

pub fn multipart(
    fields: &[(&str, String)],
    file_field: &str,
    file_name: &str,
    bytes: &[u8],
) -> Vec<u8> {
    let safe_name: String = file_name
        .chars()
        .map(|ch| {
            if ch == '"' || ch.is_control() {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let mut body = Vec::with_capacity(bytes.len() + 1024);
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{file_field}\"; filename=\"{safe_name}\"\r\nContent-Type: {}\r\n\r\n",
            content_type(file_name)
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn content_type(file_name: &str) -> &'static str {
    let lower = file_name.to_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        "image/png"
    }
}

fn form_headers(extra: &str) -> String {
    format!("Content-Type: multipart/form-data; boundary={BOUNDARY}\r\n{extra}")
}

fn text_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter()
        .try_fold(value, |node, key| node.get(key))
        .and_then(Value::as_str)
}

fn failure(host: ShareHost, status: u32, message: Option<String>) -> String {
    let name = host.label();
    match (status, message) {
        (403, _) => format!("{name} отказал в загрузке (код 403)"),
        (429, _) => format!("{name} ограничил загрузки, попробуйте позже"),
        (_, Some(message)) if !message.trim().is_empty() => {
            format!("{name} ответил: {} (код {status})", message.trim())
        }
        _ => format!("{name} ответил кодом {status}"),
    }
}

fn plain_link(host: ShareHost, status: u32, body: &str) -> Result<Uploaded, String> {
    let text = body.trim();
    if SUCCESS.contains(&status)
        && text.starts_with("https://")
        && !text.contains(char::is_whitespace)
    {
        Ok(Uploaded {
            link: text.to_owned(),
            delete: None,
        })
    } else {
        Err(failure(
            host,
            status,
            Some(text.chars().take(200).collect()),
        ))
    }
}

pub fn pixhost_image(thumb: &str) -> Option<String> {
    let rest = thumb.strip_prefix("https://t")?;
    let (server, path) = rest.split_once(".pixhost.to/thumbs/")?;
    server
        .chars()
        .all(|ch| ch.is_ascii_digit())
        .then(|| format!("https://img{server}.pixhost.to/images/{path}"))
}

pub fn parse(host: ShareHost, status: u32, body: &str) -> Result<Uploaded, String> {
    match host {
        ShareHost::Catbox => plain_link(host, status, body),
        ShareHost::Pixhost => {
            let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            let link = text_at(&value, &["th_url"])
                .and_then(pixhost_image)
                .or_else(|| text_at(&value, &["show_url"]).map(str::to_owned));
            match link {
                Some(link) if SUCCESS.contains(&status) => Ok(Uploaded { link, delete: None }),
                _ => Err(failure(
                    host,
                    status,
                    text_at(&value, &["error_msg"]).map(str::to_owned),
                )),
            }
        }
        ShareHost::ImgBb => {
            let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            match text_at(&value, &["image", "url"]) {
                Some(link) if SUCCESS.contains(&status) => Ok(Uploaded {
                    link: link.to_owned(),
                    delete: text_at(&value, &["image", "delete_url"]).map(str::to_owned),
                }),
                _ => Err(failure(
                    host,
                    status,
                    text_at(&value, &["error", "message"]).map(str::to_owned),
                )),
            }
        }
    }
}

pub fn imgbb_token(page: &str) -> Option<String> {
    let start = page.find("auth_token=\"")? + "auth_token=\"".len();
    let token: String = page[start..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    (!token.is_empty()).then_some(token)
}

fn timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis())
        .to_string()
}

pub fn upload(
    host: ShareHost,
    file_name: &str,
    bytes: &[u8],
    progress: &dyn Fn(f32),
) -> Result<Uploaded, String> {
    let response = match host {
        ShareHost::Catbox => {
            let body = multipart(
                &[("reqtype", "fileupload".to_owned())],
                "fileToUpload",
                file_name,
                bytes,
            );
            Session::new(AGENT)?.post(
                "catbox.moe",
                "/user/api.php",
                &form_headers(""),
                &body,
                progress,
            )?
        }
        ShareHost::Pixhost => {
            let body = multipart(
                &[
                    ("content_type", "0".to_owned()),
                    ("max_th_size", PIXHOST_THUMB.to_owned()),
                ],
                "img",
                file_name,
                bytes,
            );
            Session::new(AGENT)?.post(
                "api.pixhost.to",
                "/images",
                &form_headers("Accept: application/json\r\n"),
                &body,
                progress,
            )?
        }
        ShareHost::ImgBb => {
            let session = Session::new(BROWSER_AGENT)?;
            let page = session.get("imgbb.com", "/")?;
            let token = imgbb_token(&page.body).ok_or_else(|| {
                "ImgBB изменил сайт — загрузка через него сейчас не работает".to_owned()
            })?;
            let body = multipart(
                &[
                    ("type", "file".to_owned()),
                    ("action", "upload".to_owned()),
                    ("timestamp", timestamp()),
                    ("auth_token", token),
                ],
                "source",
                file_name,
                bytes,
            );
            session.post(
                "imgbb.com",
                "/json",
                &form_headers(
                    "X-Requested-With: XMLHttpRequest\r\nReferer: https://imgbb.com/\r\n",
                ),
                &body,
                progress,
            )?
        }
    };
    parse(host, response.status, &response.body)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Album {
    pub host: ShareHost,
    pub link: String,
    pub items: Vec<Uploaded>,
}

pub const fn album_host(host: ShareHost) -> ShareHost {
    match host {
        ShareHost::ImgBb => ShareHost::Catbox,
        other => other,
    }
}

fn fields_only(fields: &[(&str, String)]) -> Vec<u8> {
    let mut body = Vec::with_capacity(512);
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

pub fn catbox_name(link: &str) -> Option<&str> {
    link.strip_prefix("https://files.catbox.moe/")
        .filter(|name| !name.is_empty() && !name.contains('/'))
}

pub fn parse_gallery(body: &str) -> Option<(String, String, String)> {
    let value: Value = serde_json::from_str(body).ok()?;
    Some((
        text_at(&value, &["gallery_hash"])?.to_owned(),
        text_at(&value, &["gallery_upload_hash"])?.to_owned(),
        text_at(&value, &["gallery_url"])?.to_owned(),
    ))
}

fn share_of(index: usize, count: usize, value: f32) -> f32 {
    (index as f32 + value.clamp(0.0, 1.0)) / count.max(1) as f32
}

pub fn upload_album(
    wanted: ShareHost,
    title: &str,
    files: &[(String, Vec<u8>)],
    progress: &dyn Fn(f32),
) -> Result<Album, String> {
    let host = album_host(wanted);
    let count = files.len();
    match host {
        ShareHost::Pixhost => {
            let session = Session::new(AGENT)?;
            let created = session.post(
                "api.pixhost.to",
                "/galleries",
                &form_headers("Accept: application/json\r\n"),
                &fields_only(&[("gallery_name", title.to_owned())]),
                &|_| {},
            )?;
            let (hash, upload_hash, link) = parse_gallery(&created.body)
                .filter(|_| SUCCESS.contains(&created.status))
                .ok_or_else(|| failure(host, created.status, None))?;
            let mut items = Vec::with_capacity(count);
            for (index, (name, bytes)) in files.iter().enumerate() {
                let body = multipart(
                    &[
                        ("content_type", "0".to_owned()),
                        ("max_th_size", PIXHOST_THUMB.to_owned()),
                        ("gallery_hash", hash.clone()),
                        ("gallery_upload_hash", upload_hash.clone()),
                    ],
                    "img",
                    name,
                    bytes,
                );
                let response = session.post(
                    "api.pixhost.to",
                    "/images",
                    &form_headers("Accept: application/json\r\n"),
                    &body,
                    &|value| progress(share_of(index, count, value)),
                )?;
                items.push(parse(host, response.status, &response.body)?);
            }
            let finished = session.post(
                "api.pixhost.to",
                &format!("/galleries/{hash}/finalize"),
                &form_headers("Accept: application/json\r\n"),
                &fields_only(&[("gallery_upload_hash", upload_hash)]),
                &|_| {},
            )?;
            if !SUCCESS.contains(&finished.status) {
                return Err(failure(host, finished.status, None));
            }
            Ok(Album { host, link, items })
        }
        _ => {
            let mut items = Vec::with_capacity(count);
            for (index, (name, bytes)) in files.iter().enumerate() {
                items.push(upload(ShareHost::Catbox, name, bytes, &|value| {
                    progress(share_of(index, count, value))
                })?);
            }
            let names: Vec<&str> = items
                .iter()
                .filter_map(|item| catbox_name(&item.link))
                .collect();
            let response = Session::new(AGENT)?.post(
                "catbox.moe",
                "/user/api.php",
                &form_headers(""),
                &fields_only(&[
                    ("reqtype", "createalbum".to_owned()),
                    ("title", title.to_owned()),
                    ("desc", String::new()),
                    ("files", names.join(" ")),
                ]),
                &|_| {},
            )?;
            let link = plain_link(ShareHost::Catbox, response.status, &response.body)?.link;
            Ok(Album {
                host: ShareHost::Catbox,
                link,
                items,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_carry_the_fields_and_the_file() {
        let body = multipart(
            &[("reqtype", "fileupload".to_owned())],
            "fileToUpload",
            "shot \"1\".png",
            b"PNGDATA",
        );
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"reqtype\"\r\n\r\nfileupload\r\n"));
        assert!(text.contains("name=\"fileToUpload\"; filename=\"shot _1_.png\""));
        assert!(text.contains("Content-Type: image/png\r\n\r\nPNGDATA\r\n"));
        assert!(text.ends_with(&format!("--{BOUNDARY}--\r\n")));
    }

    #[test]
    fn every_host_reads_its_own_success_reply() {
        let cases = [
            (
                ShareHost::Catbox,
                "https://files.catbox.moe/abc.png\n",
                "https://files.catbox.moe/abc.png",
            ),
            (
                ShareHost::Pixhost,
                r#"{"name":"a.png","show_url":"https://pixhost.to/show/12/345_a.png","th_url":"https://t3.pixhost.to/thumbs/12/345_a.png"}"#,
                "https://img3.pixhost.to/images/12/345_a.png",
            ),
            (
                ShareHost::ImgBb,
                r#"{"status_code":200,"image":{"url":"https:\/\/i.ibb.co\/vx\/a.png","url_viewer":"https:\/\/ibb.co\/Fb"}}"#,
                "https://i.ibb.co/vx/a.png",
            ),
        ];
        for (host, body, link) in cases {
            assert_eq!(
                parse(host, 200, body).map(|up| up.link),
                Ok(link.to_owned()),
                "{host:?}"
            );
        }
    }

    #[test]
    fn failures_explain_themselves() {
        assert_eq!(
            parse(ShareHost::Catbox, 412, "No files uploaded"),
            Err("Catbox ответил: No files uploaded (код 412)".to_owned())
        );
        let imgbb = r#"{"status_code":400,"error":{"message":"Invalid token"}}"#;
        assert_eq!(
            parse(ShareHost::ImgBb, 400, imgbb),
            Err("ImgBB ответил: Invalid token (код 400)".to_owned())
        );
        assert_eq!(
            parse(ShareHost::Pixhost, 403, "{}"),
            Err("Pixhost отказал в загрузке (код 403)".to_owned())
        );
    }

    #[test]
    #[ignore = "uploads generated test images as an album to every host"]
    fn every_host_makes_an_album() {
        let png = |shade: u8| {
            let pixels: Vec<u8> = (0..64 * 64)
                .flat_map(|_| [0x76, shade, 0x00, 0xff])
                .collect();
            crate::views::picture::encode_png(pixels, 64, 64).unwrap()
        };
        let files = vec![
            ("rewind-album-1.png".to_owned(), png(0xb9)),
            ("rewind-album-2.png".to_owned(), png(0x40)),
        ];
        for host in ShareHost::ALL {
            let album = upload_album(*host, "Rewind test", &files, &|_| {});
            println!("{host:?}: {:?}", album.as_ref().map(|album| &album.link));
            let album = album.unwrap_or_else(|error| panic!("{host:?}: {error}"));
            assert_eq!(album.items.len(), files.len(), "{host:?}");
            assert!(album.link.starts_with("https://"), "{host:?}");
        }
    }

    #[test]
    #[ignore = "uploads a generated test image to every host"]
    fn every_host_accepts_a_generated_image() {
        let pixels: Vec<u8> = (0..64 * 64)
            .flat_map(|_| [0x76, 0xb9, 0x00, 0xff])
            .collect();
        let png = crate::views::picture::encode_png(pixels, 64, 64).unwrap();
        for host in ShareHost::ALL {
            let link = upload(*host, "rewind-test.png", &png, &|_| {});
            println!("{host:?}: {link:?}");
            assert!(link.is_ok(), "{host:?}");
        }
    }

    #[test]
    fn albums_read_their_pieces() {
        assert_eq!(
            catbox_name("https://files.catbox.moe/ab12.png"),
            Some("ab12.png")
        );
        assert_eq!(catbox_name("https://catbox.moe/c/xyz"), None);
        assert_eq!(
            parse_gallery(
                r#"{"gallery_name":"x","gallery_url":"https://pixhost.to/gallery/AbC","gallery_hash":"AbC","gallery_upload_hash":"u1"}"#
            ),
            Some((
                "AbC".to_owned(),
                "u1".to_owned(),
                "https://pixhost.to/gallery/AbC".to_owned()
            ))
        );
        assert_eq!(album_host(ShareHost::ImgBb), ShareHost::Catbox);
        assert!((share_of(1, 4, 0.5) - 0.375).abs() < f32::EPSILON);
        let form =
            String::from_utf8(fields_only(&[("reqtype", "createalbum".to_owned())])).unwrap();
        assert!(form.contains("name=\"reqtype\"\r\n\r\ncreatealbum\r\n"));
        assert!(form.ends_with(&format!("--{BOUNDARY}--\r\n")));
    }

    #[test]
    fn the_imgbb_token_is_read_from_the_page() {
        let page = r#"PF.obj.config.auth_token="2358a692839e6e41";PF.obj.config.json_api="https://imgbb.com/json";"#;
        assert_eq!(imgbb_token(page).as_deref(), Some("2358a692839e6e41"));
        assert_eq!(imgbb_token("<html></html>"), None);
        assert_eq!(pixhost_image("https://pixhost.to/x"), None);
    }
}
