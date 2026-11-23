//! Offline per-track Whisper requests and their mapping back to the meeting timeline.

use super::{mark_done, Hooks};
use crate::audio::{self, pack, speech_spans, Window};
use crate::config::Config;
use crate::model::{Segment, Track};
use crate::servers::{Kind, ModelServer};
use crate::store::Store;
use crate::whisper::{RawSegment, WhisperClient};
use anyhow::Result;
use std::time::Instant;

pub const STEP_TRACKS: &str = "tracks";

pub struct TrackCounts {
    pub you: usize,
    pub call: usize,
    pub took_s: f64,
}

pub enum TrackResult {
    Done(TrackCounts),
    Skipped(String),
}

/// Keep one server across both tracks. Offline jobs relinquish it between requests
/// when a recording needs the GPU. Sessions can lend their already-loaded client.
pub struct TrackAsr<'a> {
    borrowed: Option<&'a WhisperClient>,
    owned: Option<(ModelServer, WhisperClient)>,
}

impl<'a> TrackAsr<'a> {
    pub fn new(client: Option<&'a WhisperClient>) -> Self {
        Self {
            borrowed: client,
            owned: None,
        }
    }

    fn client(&mut self, cfg: &Config, hooks: &Hooks) -> Result<&WhisperClient> {
        if (hooks.yield_gpu)() {
            self.owned = None;
            hooks.wait_for_gpu();
        }
        if let Some(client) = self.borrowed {
            return Ok(client);
        }
        if self.owned.is_none() {
            let server = ModelServer::start(Kind::Whisper, cfg, cfg.live_devices().0)?;
            let client = WhisperClient::new(&server.base_url)?;
            self.owned = Some((server, client));
        }
        Ok(&self.owned.as_ref().expect("installed above").1)
    }
}

fn map_segment(window: &Window, raw: RawSegment, track: Track) -> Vec<Segment> {
    if raw.words.is_empty() {
        return vec![Segment {
            start: window.to_source(raw.start),
            end: window.to_source(raw.end),
            text: raw.text,
            words: vec![],
            track,
            speaker: None,
        }];
    }
    let mut groups: Vec<Segment> = Vec::new();
    let mut previous = None;
    for mut word in raw.words {
        // Assign a straddling word once. Clamp both ends to its piece so even a
        // single interpolated word cannot span the silence removed at a joint.
        let index = window.piece_at((word.start + word.end) / 2.);
        let piece = &window.pieces[index];
        word.start = piece.source_time(word.start);
        word.end = piece.source_time(word.end);
        if previous != Some(index) {
            groups.push(Segment {
                start: word.start,
                end: word.end,
                text: String::new(),
                words: Vec::new(),
                track,
                speaker: None,
            });
            previous = Some(index);
        }
        let group = groups.last_mut().expect("group for this piece");
        group.end = word.end;
        if !group.text.is_empty() {
            group.text.push(' ');
        }
        group.text.push_str(word.text.trim());
        group.words.push(word);
    }
    groups
}

pub fn transcribe_track(
    asr: &mut TrackAsr,
    track: Track,
    samples: &[i16],
    cfg: &Config,
    hooks: &Hooks,
) -> Result<Vec<Segment>> {
    let spans = speech_spans(samples, &Default::default());
    let windows = pack(&spans, samples, 30., 1.);
    let total = windows.len();
    let mut out = Vec::new();
    let mut prompt = String::new();
    for (i, window) in windows.into_iter().enumerate() {
        let client = asr.client(cfg, hooks)?;
        (hooks.progress)(STEP_TRACKS, i as f64 / total as f64);
        for raw in client.transcribe(audio::wav_bytes(&window.audio), &cfg.language, &prompt)? {
            prompt.push(' ');
            prompt.push_str(&raw.text);
            prompt = crate::session::tail_chars(&prompt, 200);
            out.extend(map_segment(&window, raw, track));
        }
    }
    (hooks.progress)(STEP_TRACKS, 1.);
    Ok(out)
}

pub fn merge_tracks(mut you: Vec<Segment>, call: Vec<Segment>) -> Vec<Segment> {
    you.extend(call);
    you.sort_by(|a, b| {
        a.start
            .total_cmp(&b.start)
            .then(a.end.total_cmp(&b.end))
            .then((a.track == Track::Call).cmp(&(b.track == Track::Call)))
    });
    you
}

pub fn transcribe_tracks(
    store: &Store,
    cfg: &Config,
    id: &str,
    client: Option<&WhisperClient>,
    hooks: &Hooks,
) -> Result<TrackResult> {
    let start = Instant::now();
    let files = store.audio_tracks(id)?;
    let meeting = store.load(id)?;
    let dir = store.dir(id);
    if meeting.meta.audio_deleted_at.is_some() || (files.mic.is_none() && files.call.is_none()) {
        return Ok(TrackResult::Skipped(
            "the audio is no longer stored — the live transcript is kept".into(),
        ));
    }
    let imported = meeting.meta.imported_from.is_some();
    let has_live = dir.join("transcript.live.jsonl").exists()
        || (dir.join("transcript.jsonl").exists() && !dir.join("transcript.tracks.jsonl").exists());
    if (files.call.is_none() && imported)
        || (!imported && has_live && (files.mic.is_none() || files.call.is_none()))
    {
        return Ok(TrackResult::Skipped(
            "only one track is stored — the live transcript is kept".into(),
        ));
    }
    let mut asr = TrackAsr::new(client);
    let sources = [
        (Track::You, files.mic.filter(|_| !imported)),
        (Track::Call, files.call),
    ];
    let total = sources.iter().filter(|(_, p)| p.is_some()).count();
    let mut lines = Vec::new();
    let mut call_lines = Vec::new();
    for (i, (track, path)) in sources
        .into_iter()
        .filter_map(|(t, p)| Some((t, p?)))
        .enumerate()
    {
        let progress =
            |step: &str, fraction| (hooks.progress)(step, (i as f64 + fraction) / total as f64);
        let samples = audio::read_wav(&path)?;
        let result = transcribe_track(
            &mut asr,
            track,
            &samples,
            cfg,
            &Hooks {
                progress: &progress,
                yield_gpu: hooks.yield_gpu,
            },
        )?;
        match track {
            Track::You => lines = result,
            Track::Call => call_lines = result,
        }
    }
    drop(asr);
    let (you, call) = (lines.len(), call_lines.len());
    let merged = merge_tracks(lines, call_lines);
    if merged.is_empty() && !meeting.segments.is_empty() {
        return Ok(TrackResult::Skipped(
            "transcribing each track found no speech — the live transcript is kept".into(),
        ));
    }
    store.keep_live(id)?;
    store.save_tracks(id, &merged)?;
    store.replace_transcript(id, &merged)?;
    store.update_meta(id, |m| m.transcript_source = None)?;
    mark_done(store, id, STEP_TRACKS)?;
    Ok(TrackResult::Done(TrackCounts {
        you,
        call,
        took_s: start.elapsed().as_secs_f64(),
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Piece;
    use crate::model::{StartInfo, TranscriptSource};
    use crate::whisper::approx_words;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const VTT: &str = "WEBVTT\n\n00:00:00.100 --> 00:00:00.500\nPaper lantern\n";
    fn server(replies: Vec<(u16, &'static str)>) -> WhisperClient {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client =
            WhisperClient::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        std::thread::spawn(move || {
            for (status, body) in replies {
                let (mut socket, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(&socket);
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                assert!(header.starts_with("POST /inference "));
                let mut len = 0;
                loop {
                    header.clear();
                    reader.read_line(&mut header).unwrap();
                    if header == "\r\n" {
                        break;
                    }
                    if let Some(n) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = n.trim().parse().unwrap();
                    }
                }
                reader.read_exact(&mut vec![0; len]).unwrap();
                write!(socket,"HTTP/1.1 {status} Response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        client
    }

    struct Fixture {
        store: Store,
        id: String,
        root: PathBuf,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    impl Fixture {
        fn new(live: bool, imported: bool, files: &[&str]) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "fofoca-tracks-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let store = Store::new(&root).unwrap();
            let id = store
                .create(&StartInfo {
                    imported_from: imported.then(|| "fixture.wav".into()),
                    ..Default::default()
                })
                .unwrap()
                .id;
            let mut audio = vec![0; 16000];
            audio[4800..11200].fill(2000);
            for file in files {
                std::fs::write(store.dir(&id).join(file), audio::wav_bytes(&audio)).unwrap();
            }
            if live {
                store
                    .append_segment(&id, &line(0., 1., Track::Call))
                    .unwrap();
            }
            Self { store, id, root }
        }
        fn run(&self, client: Option<&WhisperClient>) -> Result<TrackResult> {
            transcribe_tracks(
                &self.store,
                &Config::default(),
                &self.id,
                client,
                &Hooks {
                    progress: &|_, _| {},
                    yield_gpu: &|| false,
                },
            )
        }
        fn snapshot(&self) -> std::collections::BTreeMap<String, Vec<u8>> {
            std::fs::read_dir(self.store.dir(&self.id))
                .unwrap()
                .map(|e| {
                    let p = e.unwrap().path();
                    (
                        p.file_name().unwrap().to_str().unwrap().into(),
                        std::fs::read(p).unwrap(),
                    )
                })
                .collect()
        }
    }
    fn line(start: f64, end: f64, track: Track) -> Segment {
        Segment {
            start,
            end,
            track,
            text: "Example".into(),
            words: vec![],
            speaker: None,
        }
    }
    #[test]
    fn splits_words_at_joints_and_maps_wordless_cues() {
        let w = Window {
            audio: vec![],
            pieces: vec![
                Piece {
                    src: 16000,
                    dst: 0,
                    len: 16000,
                },
                Piece {
                    src: 160000,
                    dst: 32000,
                    len: 16000,
                },
            ],
        };
        let cue = |words| RawSegment {
            start: 0.5,
            end: 2.5,
            text: "one two".into(),
            words,
        };
        let split = map_segment(&w, cue(approx_words(0.5, 2.5, "one two")), Track::You);
        assert_eq!(
            (split[0].text.as_str(), split[0].start, split[0].end),
            ("one", 1.5, 2.)
        );
        assert_eq!(
            (split[1].text.as_str(), split[1].start, split[1].end),
            ("two", 10., 10.5)
        );
        let plain = map_segment(&w, cue(vec![]), Track::Call);
        assert_eq!((plain.len(), plain[0].start, plain[0].end), (1, 1.5, 10.5));
    }
    #[test]
    fn merge_preserves_overlaps_and_breaks_ties() {
        let a = line(1., 3., Track::You);
        let b = line(1., 3., Track::Call);
        let c = line(1., 2., Track::Call);
        assert_eq!(
            merge_tracks(vec![a.clone()], vec![b.clone(), c.clone()]),
            [c, a, b]
        );
    }
    #[test]
    fn both_tracks_and_source_round_trip_preserve_live() {
        let f = Fixture::new(true, false, &["mic.wav", "system.wav"]);
        let before = f.store.load(&f.id).unwrap().segments;
        let client = server(vec![(200, VTT); 4]);
        for _ in 0..2 {
            assert!(matches!(
                f.run(Some(&client)).unwrap(),
                TrackResult::Done(TrackCounts {
                    you: 1,
                    call: 1,
                    ..
                })
            ));
            let tracks = f
                .store
                .source_transcript(&f.id, TranscriptSource::Tracks)
                .unwrap();
            assert_eq!(tracks, f.store.load(&f.id).unwrap().segments);
            for (source, expected) in [
                (TranscriptSource::Live, &before),
                (TranscriptSource::Tracks, &tracks),
            ] {
                f.store.select_transcript(&f.id, source).unwrap();
                assert_eq!(&f.store.base_transcript(&f.id).unwrap(), expected);
                assert_eq!(&f.store.load(&f.id).unwrap().segments, expected);
            }
            assert_eq!(f.store.live_transcript(&f.id).unwrap(), before);
        }
    }
    #[test]
    fn missing_recording_tracks_leave_all_files_untouched() {
        for files in [&["system.wav"][..], &["mic.wav"], &[]] {
            let f = Fixture::new(true, false, files);
            let before = f.snapshot();
            assert!(matches!(f.run(None).unwrap(), TrackResult::Skipped(_)));
            assert_eq!(f.snapshot(), before);
        }
    }
    #[test]
    fn empty_inference_and_second_track_error_preserve_files() {
        for error in [false, true] {
            let f = Fixture::new(true, false, &["mic.wav", "system.wav"]);
            let before = f.snapshot();
            let replies = if error {
                vec![(200, VTT), (500, "failed")]
            } else {
                vec![(200, "WEBVTT"); 2]
            };
            let result = f.run(Some(&server(replies)));
            if error {
                assert!(result.is_err());
            } else {
                assert!(matches!(result, Ok(TrackResult::Skipped(_))));
            }
            assert_eq!(f.snapshot(), before);
        }
    }
    #[test]
    fn record_only_and_import_have_no_live_backup() {
        for imported in [false, true] {
            let f = Fixture::new(false, imported, &["system.wav"]);
            assert!(matches!(
                f.run(Some(&server(vec![(200, VTT)]))).unwrap(),
                TrackResult::Done(TrackCounts {
                    you: 0,
                    call: 1,
                    ..
                })
            ));
            f.store
                .replace_transcript(&f.id, &f.store.base_transcript(&f.id).unwrap())
                .unwrap();
            assert!(f
                .store
                .select_transcript(&f.id, TranscriptSource::Live)
                .is_err());
            assert!(f
                .store
                .load(&f.id)
                .unwrap()
                .meta
                .steps_done
                .iter()
                .any(|s| s == STEP_TRACKS));
        }
    }
}
