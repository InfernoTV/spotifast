//! A view of the files librespot can resolve on this computer.
//!
//! Keep discovery and playback on the same lookup: a separate tag reader
//! could produce a URI that the player cannot match back to its file.

use std::collections::{HashMap, hash_map::Entry};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use librespot_core::SpotifyUri;

use crate::api::models::{Album, ArtistRef, PlayableItem, Track};

/// Playable display rows and the matching URIs used for availability checks.
#[derive(Clone, Debug, Default)]
pub struct Library {
    pub tracks: Arc<Vec<PlayableItem>>,
    pub uris: Arc<HashMap<String, PathBuf>>,
}

/// Read the configured sources. This reads tags and must run off the UI thread.
pub fn scan(folders: &[PathBuf]) -> Result<Library, String> {
    for folder in folders {
        std::fs::read_dir(folder).map_err(|error| {
            format!(
                "Cannot read local files folder {}: {error}",
                folder.display()
            )
        })?;
    }
    let lookup = librespot_playback::local_file::create_local_file_lookup(folders);
    Ok(library(
        lookup.iter().filter(|(_, path)| playable_file(path)),
    ))
}

/// Check the playback decoder's codec, sample-rate and channel requirements
/// before offering a file in the library.
fn playable_file(path: &Path) -> bool {
    use symphonia::core::{codecs::DecoderOptions, io::MediaSourceStream, probe::Hint};

    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let source = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }
    let Ok(probed) = symphonia::default::get_probe().format(
        &hint,
        source,
        &Default::default(),
        &Default::default(),
    ) else {
        return false;
    };
    let Some(track) = probed.format.default_track() else {
        return false;
    };
    let Ok(decoder) =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())
    else {
        return false;
    };
    let params = decoder.codec_params();
    // Match the pinned player's format limits without constructing a resampling
    // filter for every file during indexing. Playback constructs that on demand.
    params
        .sample_rate
        .is_some_and(|rate| (8000..=384000).contains(&rate))
        && params.channels.is_some_and(|channels| channels.count() > 0)
}

/// Deduplicate by playback URI and sort rows by title, artist and URI so
/// repeated scans keep a stable display order.
fn library<'a>(entries: impl Iterator<Item = (&'a SpotifyUri, &'a Path)>) -> Library {
    let mut uris = HashMap::new();
    let mut tracks: Vec<_> = entries
        .filter_map(|(uri, path)| {
            let track = track(uri, path)?;
            match uris.entry(track.uri.clone()) {
                Entry::Occupied(_) => None,
                Entry::Vacant(entry) => {
                    entry.insert(path.to_path_buf());
                    Some(track)
                }
            }
        })
        .collect();
    tracks.sort_by_cached_key(|track| {
        (
            track.name.to_lowercase(),
            track.artist_names().to_lowercase(),
            track.uri.clone(),
        )
    });
    Library {
        tracks: Arc::new(tracks.into_iter().map(PlayableItem::Track).collect()),
        uris: Arc::new(uris),
    }
}

/// Build a local row from URI metadata, using the filename for a blank title.
/// Zero-length entries cannot supply the whole-second byte rate playback needs.
fn track(uri: &SpotifyUri, path: &Path) -> Option<Track> {
    let SpotifyUri::Local {
        artist,
        album_title,
        track_title,
        duration,
    } = uri
    else {
        return None;
    };
    // The local loader calculates a byte rate from whole seconds.
    if duration.as_secs() == 0 {
        return None;
    }
    let title = display_tag(track_title);
    let artist = display_tag(artist);
    let album = display_tag(album_title);
    Some(Track {
        uri: uri.to_uri(),
        name: if title.is_empty() {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            title
        },
        duration_ms: u32::try_from(duration.as_millis()).unwrap_or(u32::MAX),
        artists: (!artist.is_empty())
            .then_some(ArtistRef {
                name: artist,
                ..ArtistRef::default()
            })
            .into_iter()
            .collect(),
        album: (!album.is_empty()).then_some(Album {
            name: album,
            ..Album::default()
        }),
        is_local: true,
        ..Track::default()
    })
}

/// Decode the form-encoded tag components in librespot's local URIs.
fn display_tag(tag: &str) -> String {
    percent_encoding::percent_decode_str(&tag.replace('+', " "))
        .decode_utf8_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_real_tags_and_accepts_supported_channels() {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testdata/local-files");
        let found = scan(&[folder.clone(), folder]).unwrap();
        let item = found
            .tracks
            .iter()
            .find(|item| item.name() == "Test Song")
            .expect("the tagged 44.1 kHz stereo fixture is indexed");
        assert_eq!(item.name(), "Test Song");
        assert_eq!(item.duration_ms(), 2_000);
        assert_eq!(
            item.uri(),
            "spotify:local:Test+Artist:Test+Album:Test+Song:2"
        );
        assert!(found.uris.contains_key(item.uri()));
        assert_eq!(found.uris.len(), found.tracks.len());
        assert_eq!(found.tracks.len(), 3);
        assert_eq!(
            found.uris[item.uri()].file_name().unwrap(),
            "stereo-44100.flac"
        );
        assert!(
            found
                .tracks
                .iter()
                .any(|item| item.name() == "Unsupported Rate")
        );
        assert!(
            found
                .tracks
                .iter()
                .any(|item| item.name() == "Unsupported Channels")
        );
    }

    fn local(uri: &str) -> SpotifyUri {
        SpotifyUri::from_uri(uri).unwrap()
    }

    #[test]
    fn lookup_metadata_stays_local_and_decodes_tags() {
        let uri = local("spotify:local:Bj%C3%B6rk:Home+Recordings:A%2BB%3A+C:180");
        let track = track(&uri, Path::new("music.mp3")).unwrap();
        assert_eq!(track.uri, uri.to_uri());
        assert_eq!(track.name, "A+B: C");
        assert_eq!(track.artist_names(), "Björk");
        assert_eq!(track.album.unwrap().name, "Home Recordings");
        assert_eq!(track.duration_ms, 180_000);
        assert!(track.id.is_none());
        assert!(track.is_local);
    }

    #[test]
    fn duplicate_sources_keep_one_row_and_a_stable_order() {
        let first = local("spotify:local:Artist:Album:alpha:100");
        let last = local("spotify:local:Artist:Album:Zulu:100");
        let entries = [
            (&last, Path::new("z.mp3")),
            (&first, Path::new("a.mp3")),
            (&last, Path::new("duplicate.mp3")),
        ];
        let found = library(entries.into_iter());
        assert_eq!(found.tracks.len(), 2);
        assert_eq!(found.uris.len(), 2);
        assert_eq!(found.tracks[0].uri(), first.to_uri());
        assert_eq!(found.tracks[1].uri(), last.to_uri());
    }

    #[test]
    fn missing_title_uses_the_filename_and_zero_length_is_skipped() {
        let untitled = local("spotify:local::::180");
        assert_eq!(
            track(&untitled, Path::new("Recording.mp3")).unwrap().name,
            "Recording"
        );
        assert!(track(&local("spotify:local::::0"), Path::new("empty.mp3")).is_none());
    }

    #[test]
    fn an_unreadable_source_returns_a_visible_error() {
        let missing = std::env::temp_dir().join(format!(
            "spotifast-missing-local-source-{}",
            std::process::id()
        ));
        let error = scan(std::slice::from_ref(&missing)).unwrap_err();
        assert!(error.contains(&missing.display().to_string()));
        assert!(error.starts_with("Cannot read local files folder"));
        assert!(scan(&[]).unwrap().tracks.is_empty());
    }

    #[test]
    fn corrupt_audio_is_not_offered_as_playable() {
        let path = std::env::temp_dir().join(format!(
            "spotifast-corrupt-local-{}-{}.mp3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"This is not an MP3").unwrap();
        assert!(!playable_file(&path));
        std::fs::remove_file(path).unwrap();
    }
}
