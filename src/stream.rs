//! Songs from YouTube, the way the Local AI music widget finds them: look a
//! title up (local SearXNG if it's running, otherwise YouTube's own results
//! page, otherwise yt-dlp's search), then stream the audio through yt-dlp
//! into memory. Nothing is written to disk.

use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;

use crate::library::Track;

const SEARXNG: &str = "http://127.0.0.1:8888";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";
/// Streams kept in memory at once (current, next, and a prefetched result).
const KEEP: usize = 4;
/// Bytes to have before playback starts (~15 s of 128 kbps AAC).
pub const START_BYTES: usize = 256 * 1024;

// ---------------------------------------------------------------- tracks

pub const PREFIX: &str = "youtube:";

pub fn track(id: &str, raw_title: &str, channel: &str, duration_ms: u32) -> Track {
    let (title, artist) = split_title(raw_title, channel);
    Track {
        path: PathBuf::from(format!("{PREFIX}{id}")),
        title: title.into(),
        artist: artist.clone().into(),
        album: "".into(),
        album_artist: artist.into(),
        track_no: 0,
        disc_no: 0,
        year: 0,
        duration_ms,
        mtime: 0,
    }
}

/// YouTube id of a streamed track.
pub fn id_of(track: &Track) -> Option<&str> {
    track.path.to_str()?.strip_prefix(PREFIX)
}

/// "Artist - Title (Official Video)" → ("Title", "Artist"); otherwise the
/// channel (minus YouTube's " - Topic") is the artist.
fn split_title(raw: &str, channel: &str) -> (String, String) {
    let mut t = raw.trim().trim_end_matches(" - YouTube").to_string();
    for (open, close) in [('(', ')'), ('[', ']')] {
        while let Some(a) = t.find(open) {
            let Some(b) = t[a..].find(close).map(|b| a + b) else { break };
            let inner = t[a + 1..b].to_lowercase();
            let noise = ["official", "lyric", "audio", "video", "visuali", "hd", "4k", "hq", "remaster", "mv"];
            if !noise.iter().any(|n| inner.contains(n)) {
                break;
            }
            t.replace_range(a..=b, "");
        }
    }
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let channel = channel.trim().trim_end_matches(" - Topic").trim_end_matches("VEVO").trim();
    for sep in [" - ", " – ", " — ", " | "] {
        if let Some((a, b)) = t.split_once(sep) {
            return (b.trim().trim_matches('"').to_string(), a.trim().to_string());
        }
    }
    (t, channel.to_string())
}

/// Id from any of the usual YouTube link shapes.
pub fn link_id(s: &str) -> Option<String> {
    let s = s.trim();
    let rest = ["youtube.com/watch?", "youtube-nocookie.com/embed/", "youtube.com/embed/", "youtube.com/shorts/", "youtube.com/live/", "youtu.be/"]
        .iter()
        .find_map(|m| s.find(m).map(|i| &s[i + m.len()..]))?;
    let rest = if s.contains("watch?") {
        rest.split('&').find_map(|kv| kv.strip_prefix("v="))?
    } else {
        rest
    };
    let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    (id.len() == 11).then_some(id)
}

fn parse_len(s: &str) -> u32 {
    s.split(':').try_fold(0u32, |acc, p| p.trim().parse::<u32>().ok().map(|v| acc * 60 + v)).unwrap_or(0) * 1000
}

// ---------------------------------------------------------------- search

/// Look a song up. Accepts a title or a YouTube link.
pub fn search(query: &str) -> Result<Vec<Track>, String> {
    let q = query.trim();
    if let Some(id) = link_id(q) {
        return Ok(vec![oembed(&id).unwrap_or_else(|| track(&id, "YouTube", "", 0))]);
    }
    // Songs, not documentaries: drop long videos unless the query asks for them.
    let long_ok = ["mix", "album", "full", "hour", "live", "set", "concert", "playlist", "ost", "soundtrack"]
        .iter()
        .any(|w| q.to_lowercase().split_whitespace().any(|t| t == *w));
    let keep = |v: Vec<Track>| -> Vec<Track> {
        if long_ok {
            return v;
        }
        let short: Vec<Track> = v.iter().filter(|t| t.duration_ms == 0 || t.duration_ms <= 15 * 60_000).cloned().collect();
        if short.is_empty() { v } else { short }
    };
    let mut errors = Vec::new();
    for (name, f) in [("SearXNG", searxng as fn(&str) -> Result<Vec<Track>, String>), ("YouTube", results_page), ("yt-dlp", ytdlp_search)] {
        match f(q) {
            Ok(v) if !v.is_empty() => return Ok(keep(v)),
            Ok(_) => errors.push(format!("{name}: nothing found")),
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    Err(errors.pop().unwrap_or_else(|| "nothing found".into()))
}

fn agent() -> ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_millis(1500)))
                .timeout_global(Some(Duration::from_secs(12)))
                .build()
                .into()
        })
        .clone()
}

fn get_string(url: &str, query: &[(&str, &str)]) -> Result<String, String> {
    let mut req = agent()
        .get(url)
        .header("User-Agent", UA)
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Cookie", "CONSENT=YES+1; SOCS=CAI");
    for (k, v) in query {
        req = req.query(*k, *v);
    }
    let mut resp = req.call().map_err(|e| e.to_string())?;
    resp.body_mut().with_config().limit(8 << 20).read_to_string().map_err(|e| e.to_string())
}

fn searxng(q: &str) -> Result<Vec<Track>, String> {
    let body = get_string(&format!("{SEARXNG}/search"), &[("q", &format!("!yt {q}")), ("format", "json")])?;
    let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let mut out: Vec<Track> = Vec::new();
    for r in v["results"].as_array().into_iter().flatten() {
        let Some(id) = r["url"].as_str().and_then(link_id) else { continue };
        if out.iter().any(|t| id_of(t) == Some(&id)) {
            continue;
        }
        let len = r["length"].as_str().map(parse_len).unwrap_or(0);
        out.push(track(&id, r["title"].as_str().unwrap_or(""), r["author"].as_str().unwrap_or(""), len));
    }
    Ok(out)
}

/// Parse the JSON YouTube embeds in its results page.
fn results_page(q: &str) -> Result<Vec<Track>, String> {
    // sp=EgIQAQ== restricts results to videos (no channels/playlists).
    let html = get_string("https://www.youtube.com/results", &[("search_query", q), ("sp", "EgIQAQ==")])?;
    let start = html.find("var ytInitialData = ").ok_or("unexpected page")? + "var ytInitialData = ".len();
    let data: Value = serde_json::Deserializer::from_str(&html[start..])
        .into_iter::<Value>()
        .next()
        .ok_or("unexpected page")?
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    collect_videos(&data, &mut out);
    Ok(out)
}

fn collect_videos(v: &Value, out: &mut Vec<Track>) {
    if out.len() >= 15 {
        return;
    }
    match v {
        Value::Object(map) => {
            if let Some(r) = map.get("videoRenderer") {
                let text = |v: &Value| v["runs"][0]["text"].as_str().or(v["simpleText"].as_str()).unwrap_or("").to_string();
                let len = r["lengthText"]["simpleText"].as_str().map(parse_len).unwrap_or(0);
                if let (Some(id), true) = (r["videoId"].as_str(), len > 0) {
                    out.push(track(id, &text(&r["title"]), &text(&r["ownerText"]), len));
                }
                return;
            }
            for v in map.values() {
                collect_videos(v, out);
            }
        }
        Value::Array(a) => {
            for v in a {
                collect_videos(v, out);
            }
        }
        _ => {}
    }
}

fn ytdlp_search(q: &str) -> Result<Vec<Track>, String> {
    let out = ytdlp()?
        .args(["--flat-playlist", "--no-warnings", "--print", "%(id)s\t%(title)s\t%(channel)s\t%(duration)s"])
        .arg(format!("ytsearch12:{q}"))
        .stderr(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let (id, title, channel, dur) = (f.next()?, f.next()?, f.next()?, f.next()?);
            let secs = dur.parse::<f32>().ok().filter(|s| *s > 0.0)?;
            Some(track(id, title, channel, (secs * 1000.0) as u32))
        })
        .collect())
}

/// Title and channel for a pasted link.
fn oembed(id: &str) -> Option<Track> {
    let url = format!("https://www.youtube.com/watch?v={id}");
    let body = get_string("https://www.youtube.com/oembed", &[("url", &url), ("format", "json")]).ok()?;
    let v: Value = serde_json::from_str(&body).ok()?;
    Some(track(id, v["title"].as_str()?, v["author_name"].as_str().unwrap_or(""), 0))
}

/// Cover for a streamed track: YouTube's thumbnail with the letterbox cropped.
pub fn thumbnail(id: &str) -> Option<image::DynamicImage> {
    let mut resp = agent().get(&format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg")).call().ok()?;
    let bytes = resp.body_mut().read_to_vec().ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    // hqdefault is 4:3 with black bars around 16:9 video.
    let bar = (h - w * 9 / 16) / 2;
    Some(if bar > 0 && bar < h / 4 { img.crop_imm(0, bar, w, h - bar * 2) } else { img })
}

// --------------------------------------------------------------- yt-dlp

/// Official single-file yt-dlp build, fetched once when Wax ships as a lone exe.
const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";

/// yt-dlp, looked for next to the app (`tools/`), in Wax's data folder, then on PATH.
fn find_ytdlp() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        roots.extend(exe.ancestors().skip(1).take(4).map(Into::into));
    }
    roots.push(crate::library::data_dir());
    for root in roots {
        for rel in ["tools/yt-dlp/yt-dlp.exe", "tools/yt-dlp.exe", "yt-dlp/yt-dlp.exe", "yt-dlp.exe"] {
            let p = root.join(rel);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let mut on_path = Command::new("yt-dlp");
    on_path.arg("--version").stdout(Stdio::null()).stderr(Stdio::null());
    hide(&mut on_path).status().ok().filter(|s| s.success()).map(|_| PathBuf::from("yt-dlp"))
}

/// Downloads yt-dlp.exe into Wax's data folder (written to a temp name, then renamed).
#[cfg(windows)]
fn download_ytdlp() -> Result<PathBuf, String> {
    let dest = crate::library::data_dir().join("yt-dlp.exe");
    let part = dest.with_extension("exe.part");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .into();
    let mut body = agent
        .get(YTDLP_URL)
        .header("User-Agent", "wax")
        .call()
        .map_err(|e| format!("couldn't download yt-dlp: {e}"))?
        .into_body();
    let mut reader = body.as_reader();
    let mut file = std::fs::File::create(&part).map_err(|e| format!("couldn't save yt-dlp: {e}"))?;
    let n = std::io::copy(&mut reader, &mut file).map_err(|e| format!("couldn't download yt-dlp: {e}"))?;
    drop(file);
    if n < 1_000_000 {
        let _ = std::fs::remove_file(&part);
        return Err("yt-dlp download came back incomplete".into());
    }
    std::fs::rename(&part, &dest).map_err(|e| format!("couldn't save yt-dlp: {e}"))?;
    Ok(dest)
}

#[cfg(not(windows))]
fn download_ytdlp() -> Result<PathBuf, String> {
    Err("yt-dlp isn't installed (install it or put it on PATH)".into())
}

/// The yt-dlp to run, downloading it on first use if it's nowhere to be found.
pub fn ytdlp_path() -> Result<PathBuf, String> {
    static PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
    let mut slot = PATH.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = slot.as_ref() {
        return Ok(p.clone());
    }
    let p = match find_ytdlp() {
        Some(p) => p,
        None => download_ytdlp()?,
    };
    *slot = Some(p.clone());
    Ok(p)
}

fn ytdlp() -> Result<Command, String> {
    let path = ytdlp_path()?;
    let mut cmd = Command::new(path);
    hide(&mut cmd);
    Ok(cmd)
}

fn hide(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

// --------------------------------------------------------------- buffers

#[derive(Default)]
struct BufState {
    data: Vec<u8>,
    done: bool,
    failed: Option<String>,
}

/// Audio bytes arriving from yt-dlp; readers block until what they need is here.
#[derive(Default)]
pub struct StreamBuf {
    state: Mutex<BufState>,
    grew: Condvar,
}

pub enum Ready {
    Partial,
    Done,
    Failed(String),
}

impl StreamBuf {
    pub fn status(&self) -> Ready {
        let s = self.state.lock().unwrap();
        match (&s.failed, s.done) {
            (Some(e), _) => Ready::Failed(e.clone()),
            (None, true) => Ready::Done,
            _ => Ready::Partial,
        }
    }

    /// Block until at least `bytes` have arrived or the download ended.
    pub fn wait_for(&self, bytes: usize, timeout: Duration) -> Ready {
        let s = self.state.lock().unwrap();
        let (s, _) = self
            .grew
            .wait_timeout_while(s, timeout, |s| s.data.len() < bytes && !s.done && s.failed.is_none())
            .unwrap();
        match (&s.failed, s.done, s.data.len() >= bytes) {
            (Some(e), _, _) => Ready::Failed(e.clone()),
            (None, true, _) => Ready::Done,
            (None, false, true) => Ready::Partial,
            _ => Ready::Failed("timed out waiting for audio".into()),
        }
    }

    pub fn len(&self) -> usize {
        self.state.lock().unwrap().data.len()
    }

    pub fn reader(self: &Arc<Self>) -> StreamReader {
        StreamReader { buf: self.clone(), pos: 0 }
    }
}

pub struct StreamReader {
    buf: Arc<StreamBuf>,
    pos: u64,
}

impl Read for StreamReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let want = self.pos as usize + 1;
        let s = self.buf.state.lock().unwrap();
        let s = self.buf.grew.wait_while(s, |s| s.data.len() < want && !s.done && s.failed.is_none()).unwrap();
        let start = (self.pos as usize).min(s.data.len());
        let n = out.len().min(s.data.len() - start);
        out[..n].copy_from_slice(&s.data[start..start + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for StreamReader {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.pos = match to {
            SeekFrom::Start(p) => p,
            SeekFrom::Current(d) => (self.pos as i64 + d).max(0) as u64,
            SeekFrom::End(d) => {
                let s = self.buf.state.lock().unwrap();
                let s = self.buf.grew.wait_while(s, |s| !s.done && s.failed.is_none()).unwrap();
                (s.data.len() as i64 + d).max(0) as u64
            }
        };
        Ok(self.pos)
    }
}

/// Start (or reuse) the download of a track's audio.
pub fn fetch(id: &str) -> Arc<StreamBuf> {
    static CACHE: OnceLock<Mutex<VecDeque<(String, Arc<StreamBuf>)>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    if let Some(i) = cache.iter().position(|(k, b)| k == id && !matches!(b.status(), Ready::Failed(_))) {
        let entry = cache.remove(i).unwrap();
        let buf = entry.1.clone();
        cache.push_back(entry);
        return buf;
    }
    let buf = Arc::new(StreamBuf::default());
    cache.retain(|(k, _)| k != id);
    cache.push_back((id.to_owned(), buf.clone()));
    while cache.len() > KEEP {
        cache.pop_front();
    }
    let (b, id) = (buf.clone(), id.to_owned());
    std::thread::Builder::new()
        .name("wax-fetch".into())
        .spawn(move || {
            let result = download(&id, &b);
            let mut s = b.state.lock().unwrap();
            match result {
                Ok(()) if !s.data.is_empty() => s.done = true,
                Ok(()) => s.failed = Some("YouTube sent no audio".into()),
                Err(e) => s.failed = Some(e),
            }
            s.data.shrink_to_fit();
            drop(s);
            b.grew.notify_all();
        })
        .expect("spawn fetch thread");
    buf
}

fn download(id: &str, buf: &StreamBuf) -> Result<(), String> {
    let mut child = ytdlp()?
        // AAC in MP4: the decoder has no Opus support.
        .args(["-f", "bestaudio[ext=m4a]/bestaudio[acodec^=mp4a]", "--no-playlist", "--no-warnings", "--quiet", "--no-part", "-o", "-"])
        .arg(format!("https://www.youtube.com/watch?v={id}"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().unwrap();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = stdout.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        buf.state.lock().unwrap().data.extend_from_slice(&chunk[..n]);
        buf.grew.notify_all();
    }
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        let line = err.lines().rev().find(|l| l.contains("ERROR")).unwrap_or("yt-dlp failed").to_string();
        let line = line.split("] ").last().unwrap_or(&line).trim_start_matches("ERROR: ");
        return Err(friendly(line));
    }
    Ok(())
}

fn friendly(e: &str) -> String {
    let l = e.to_lowercase();
    if l.contains("sign in") || l.contains("age") {
        "YouTube wants a sign-in for this one".into()
    } else if l.contains("not available") || l.contains("unavailable") {
        "not available".into()
    } else {
        e.chars().take(80).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(split_title("Daft Punk - Something About Us (Official Video)", "Daft Punk"), ("Something About Us".into(), "Daft Punk".into()));
        assert_eq!(split_title("Roygbiv", "Boards of Canada - Topic"), ("Roygbiv".into(), "Boards of Canada".into()));
        assert_eq!(split_title("Song (Live at X)", "Band"), ("Song (Live at X)".into(), "Band".into()));
    }

    #[test]
    fn links() {
        assert_eq!(link_id("https://www.youtube.com/watch?v=sOS9aOIXPEk&t=3").as_deref(), Some("sOS9aOIXPEk"));
        assert_eq!(link_id("https://youtu.be/sOS9aOIXPEk").as_deref(), Some("sOS9aOIXPEk"));
        assert_eq!(link_id("daft punk"), None);
        assert_eq!(parse_len("3:49"), 229_000);
        assert_eq!(parse_len("1:02:03"), 3_723_000);
    }
}
