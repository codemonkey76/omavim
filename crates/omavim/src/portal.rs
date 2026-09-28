//! The desktop through xdg-desktop-portal: file pickers, printing, and the
//! dark/light and text size settings. What a full toolkit would give for free, Omavim asks the portal
//! for (the same portals Omawrite relies on).

use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};
use ashpd::desktop::settings::{ColorScheme, Settings};
use futures::{SinkExt, Stream, StreamExt};
use std::path::{Path, PathBuf};

/// Dark or light, as the desktop wants it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scheme {
    #[default]
    Light,
    Dark,
}

impl From<ColorScheme> for Scheme {
    fn from(c: ColorScheme) -> Self {
        match c {
            ColorScheme::PreferDark => Scheme::Dark,
            // No preference means the desktop's default, which is light.
            ColorScheme::PreferLight | ColorScheme::NoPreference => Scheme::Light,
        }
    }
}

/// The desktop's colour scheme now, then every time it changes. Nothing at
/// all if the portal can't be reached: the app stays light.
pub fn color_scheme() -> impl Stream<Item = Scheme> {
    iced::stream::channel(4, async |mut out| {
        let Ok(settings) = Settings::new().await else {
            return;
        };
        if let Ok(now) = settings.color_scheme().await {
            let _ = out.send(now.into()).await;
        }
        let Ok(mut changes) = settings.receive_color_scheme_changed().await else {
            return;
        };
        while let Some(scheme) = changes.next().await {
            if out.send(scheme.into()).await.is_err() {
                return;
            }
        }
    })
}

/// The desktop's text size, as a factor (1.0 is the design size), now and
/// every time it changes. `omarchy display text size` and GNOME's Text Size
/// both set it (GNOME's `text-scaling-factor`). Nothing if the portal can't
/// be reached or doesn't have it: the app stays at 1.0.
pub fn text_scale() -> impl Stream<Item = f32> {
    const NAMESPACE: &str = "org.gnome.desktop.interface";
    const KEY: &str = "text-scaling-factor";
    iced::stream::channel(4, async |mut out| {
        let Ok(settings) = Settings::new().await else {
            return;
        };
        if let Ok(now) = settings.read::<f64>(NAMESPACE, KEY).await {
            let _ = out.send(now as f32).await;
        }
        let Ok(mut changes) = settings
            .receive_setting_changed_with_args::<f64>(NAMESPACE, KEY)
            .await
        else {
            return;
        };
        while let Some(scale) = changes.next().await {
            if let Ok(scale) = scale
                && out.send(scale as f32).await.is_err()
            {
                return;
            }
        }
    })
}

fn text_files() -> FileFilter {
    FileFilter::new("Text")
        .mimetype("text/plain")
        .mimetype("text/markdown")
        .glob("*.md")
        .glob("*.txt")
}

/// Ask for a file to open, and read it. `Ok(None)` if the picker was cancelled.
pub async fn open() -> Result<Option<(PathBuf, String)>, String> {
    let request = SelectedFiles::open_file()
        .title("Open")
        .modal(true)
        .filter(text_files())
        .filter(FileFilter::new("All files").glob("*"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let Some(path) = chosen(request.response())? else {
        return Ok(None);
    };
    let contents = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some((path, contents)))
}

/// Ask where to save, suggesting `name`. `Ok(None)` if cancelled.
pub async fn save_as(name: String, folder: Option<PathBuf>) -> Result<Option<PathBuf>, String> {
    let mut request = SelectedFiles::save_file()
        .title("Save as")
        .modal(true)
        .current_name(name.as_str());
    if let Some(folder) = folder {
        request = request.current_folder(folder).map_err(|e| e.to_string())?;
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    chosen(response.response())
}

/// Write the text to `path`: to a temporary file beside it, then renamed over
/// it, so a crash mid-write can't leave half a document.
pub async fn write(path: PathBuf, text: String) -> Result<PathBuf, String> {
    let tmp = sibling(&path, ".omavim-save");
    tokio::fs::write(&tmp, text.as_bytes())
        .await
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// The picked file as a local path. Cancelling isn't an error.
/// The print dialog: which printer, and the paper and its margins. `Ok(None)`
/// if it was cancelled; else the paper and a token for [`print`].
pub async fn prepare_print(title: String) -> Result<Option<(crate::print::Paper, u32)>, String> {
    use ashpd::desktop::print::{Orientation, PreparePrintOptions, PrintProxy};
    let proxy = PrintProxy::new().await.map_err(|e| e.to_string())?;
    let request = proxy
        .prepare_print(
            None,
            &title,
            Default::default(),
            Default::default(),
            PreparePrintOptions::default().set_modal(true),
        )
        .await
        .map_err(|e| e.to_string())?;
    let setup = match request.response() {
        Ok(setup) => setup,
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let p = setup.page_setup;
    let (mut width, mut height) = (p.width.unwrap_or(210.0), p.height.unwrap_or(297.0));
    let mut margins =
        [p.margin_top, p.margin_right, p.margin_bottom, p.margin_left].map(|m| m.unwrap_or(0.0));
    // (The paper's size is given upright: turned for landscape.)
    if matches!(
        p.orientation,
        Some(Orientation::Landscape | Orientation::ReverseLandscape)
    ) {
        std::mem::swap(&mut width, &mut height);
        margins.rotate_right(1);
    }
    Ok(Some((
        crate::print::Paper::from_mm(width, height, margins),
        setup.token,
    )))
}

/// Print a PDF, with the dialog's token.
pub async fn print(title: String, pdf: Vec<u8>, token: u32) -> Result<(), String> {
    use ashpd::desktop::print::{PrintOptions, PrintProxy};
    use std::os::fd::AsFd;
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = dir.join(format!("omavim-print-{}.pdf", std::process::id()));
    tokio::fs::write(&path, &pdf)
        .await
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let file = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    // (Open, it can go from the folder: the portal reads it through the fd.)
    let _ = std::fs::remove_file(&path);
    let proxy = PrintProxy::new().await.map_err(|e| e.to_string())?;
    let request = proxy
        .print(
            None,
            &title,
            &file.as_fd(),
            PrintOptions::default().set_token(token).set_modal(true),
        )
        .await
        .map_err(|e| e.to_string())?;
    match request.response() {
        Ok(()) | Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn chosen(response: Result<SelectedFiles, ashpd::Error>) -> Result<Option<PathBuf>, String> {
    match response {
        Ok(files) => Ok(files.uris().first().and_then(|u| file_path(u.as_str()))),
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// `file:///home/me/My%20notes.md` → `/home/me/My notes.md`.
fn file_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn turns_a_file_uri_into_a_path() {
        assert_eq!(
            super::file_path("file:///home/me/My%20notes.md")
                .unwrap()
                .to_str(),
            Some("/home/me/My notes.md")
        );
        assert_eq!(
            super::file_path("file:///tmp/caf%C3%A9.md")
                .unwrap()
                .to_str(),
            Some("/tmp/café.md")
        );
        assert!(super::file_path("https://example.com/x").is_none());
    }
}
