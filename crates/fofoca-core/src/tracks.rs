//! Putting the two transcribed tracks back together (spec 018).
//!
//! The mic (`You`) and the call (`Call`) are transcribed separately, so a line's track comes from
//! the file it was transcribed from. This merges the two lists by time and drops mic lines that
//! only repeat the call: without headphones the mic hears the speakers, and Whisper would
//! transcribe that copy as You.

use crate::audio;
use crate::model::{Segment, Track, Word};
use crate::whisper::RawSegment;

/// A mic line is an echo when a call line overlaps it for at least this share of its length…
const ECHO_OVERLAP: f64 = 0.5;
/// …and at least this share of its normalised words appear in that call line.
const ECHO_WORDS: f64 = 0.6;

/// The tracks of a chunk worth a Whisper request, mic first (the order they go to the GPU). A
/// track without speech costs nothing.
pub fn with_speech<'a>(mic: &'a [i16], sys: &'a [i16]) -> Vec<(Track, &'a [i16])> {
    [(Track::You, mic), (Track::Call, sys)].into_iter().filter(|(_, s)| audio::has_speech(s)).collect()
}

/// One chunk, transcribed per track.
#[derive(Debug, Default)]
pub struct ChunkLines {
    /// Both tracks' lines in meeting time, merged by start, echoes dropped.
    pub segments: Vec<Segment>,
    /// Mic lines dropped as echoes of the call.
    pub echoes: usize,
    /// Tracks whose request failed; the other track's lines are still kept.
    pub errors: Vec<(Track, anyhow::Error)>,
}

/// Send each part to `transcribe` (one Whisper request per track, in order), label its lines
/// with the part's track, move them to meeting time (`offset` = chunk start, s) and merge.
pub fn transcribe_parts(
    parts: &[(Track, &[i16])],
    offset: f64,
    mut transcribe: impl FnMut(&[i16]) -> anyhow::Result<Vec<RawSegment>>,
) -> ChunkLines {
    let (mut mic, mut call, mut errors) = (Vec::new(), Vec::new(), Vec::new());
    for &(track, samples) in parts {
        match transcribe(samples) {
            Ok(raw) => {
                let lines = raw.into_iter().map(|r| segment(r, track, offset));
                match track {
                    Track::You => mic.extend(lines),
                    Track::Call => call.extend(lines),
                }
            }
            Err(e) => errors.push((track, e)),
        }
    }
    let (segments, echoes) = merge(mic, call);
    ChunkLines { segments, echoes, errors }
}

/// A Whisper line from `track`, moved by `offset` seconds.
pub fn segment(r: RawSegment, track: Track, offset: f64) -> Segment {
    Segment {
        start: offset + r.start,
        end: offset + r.end,
        words: r.words.into_iter().map(|w| Word { start: offset + w.start, end: offset + w.end, ..w }).collect(),
        text: r.text,
        track,
        speaker: None,
    }
}

/// Both tracks' lines in time order, without mic echoes of the call. Returns the merged lines and
/// how many mic lines were dropped as echoes. Lines starting together keep mic before call.
pub fn merge(mic: Vec<Segment>, call: Vec<Segment>) -> (Vec<Segment>, usize) {
    let before = mic.len();
    let mic: Vec<Segment> = mic.into_iter().filter(|m| !call.iter().any(|c| is_echo(m, c))).collect();
    let dropped = before - mic.len();
    let mut out = mic;
    out.extend(call);
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    (out, dropped)
}

/// Whether mic line `mic` is the call line `call` heard through the speakers.
pub fn is_echo(mic: &Segment, call: &Segment) -> bool {
    let len = mic.end - mic.start;
    let overlap = mic.end.min(call.end) - mic.start.max(call.start);
    let overlaps = if len > 0.0 { overlap >= ECHO_OVERLAP * len } else { overlap >= 0.0 };
    if !overlaps {
        return false;
    }
    let mine = words(&mic.text);
    if mine.is_empty() {
        return false;
    }
    let theirs = words(&call.text);
    let found = mine.iter().filter(|w| theirs.contains(w)).count();
    found as f64 >= ECHO_WORDS * mine.len() as f64
}

/// Lower-case words without accents or punctuation: `"Não, é."` → `["nao", "e"]`.
fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).map(unaccent).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect()
}

fn unaccent(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        c => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Track;

    fn line(track: Track, start: f64, end: f64, text: &str) -> Segment {
        Segment { start, end, text: text.into(), track, words: vec![], speaker: None }
    }

    #[test]
    fn normalises_case_accents_and_punctuation() {
        assert_eq!(words("Não, É  ação!"), vec!["nao", "e", "acao"]);
        assert!(words(" ... ").is_empty());
    }

    #[test]
    fn exact_echo_is_dropped() {
        let call = line(Track::Call, 10.0, 13.0, "A gente fecha o relatório na sexta.");
        let echo = line(Track::You, 10.2, 13.1, "a gente fecha o relatório na sexta");
        assert!(is_echo(&echo, &call));
        let (merged, dropped) = merge(vec![echo], vec![call.clone()]);
        assert_eq!((merged, dropped), (vec![call], 1));
    }

    #[test]
    fn partial_echo_is_dropped() {
        // Whisper heard most of the call line on the mic, with one word wrong and one missing.
        let call = line(Track::Call, 20.0, 24.0, "Então a gente manda o contrato amanhã cedo");
        let echo = line(Track::You, 20.5, 23.5, "então gente manda o contato amanhã cedo");
        assert!(is_echo(&echo, &call), "6 of 7 words match");
    }

    #[test]
    fn overlapping_reply_is_kept() {
        // talking over each other: different words at the same time are two people
        let call = line(Track::Call, 30.0, 33.0, "Você consegue mandar até sexta?");
        let reply = line(Track::You, 31.0, 33.5, "Sim, mando hoje mesmo.");
        assert!(!is_echo(&reply, &call));
        let (merged, dropped) = merge(vec![reply.clone()], vec![call.clone()]);
        assert_eq!((merged, dropped), (vec![call, reply], 0));
    }

    #[test]
    fn same_word_at_another_time_is_kept() {
        let call = line(Track::Call, 40.0, 40.6, "Sim.");
        let you = line(Track::You, 42.0, 42.5, "sim");
        assert!(!is_echo(&you, &call));
        // a short overlap isn't enough either
        let late = line(Track::You, 40.4, 41.4, "sim");
        assert!(!is_echo(&late, &call), "20 % overlap");
    }

    const SR: usize = audio::SAMPLE_RATE as usize;

    /// Voice-like audio (150 ms syllables, 100 ms gaps) between `a` and `b` seconds of `secs`.
    fn voice(secs: f64, a: f64, b: f64, amp: i16) -> Vec<i16> {
        let (on, period) = (SR * 15 / 100, SR / 4);
        (0..(secs * SR as f64) as usize)
            .map(|i| {
                let t = i as f64 / SR as f64;
                if t >= a && t < b && i % period < on { if i % 2 == 0 { amp } else { -amp } } else { 0 }
            })
            .collect()
    }

    /// A stand-in for Whisper: one line over the voiced part of what it was sent.
    fn fake_whisper(text: &'static str) -> impl Fn(&[i16]) -> Vec<RawSegment> {
        move |s: &[i16]| {
            let voiced: Vec<usize> = s.chunks(SR / 20).enumerate().filter(|(_, w)| audio::rms(w) > 0.01).map(|(i, _)| i).collect();
            let (a, b) = (voiced[0] as f64 / 20.0, (voiced[voiced.len() - 1] + 1) as f64 / 20.0);
            vec![RawSegment { start: a, end: b, text: text.into(), words: crate::whisper::approx_words(a, b, text) }]
        }
    }

    /// The bug that started spec 018 (00:00 of the measured call): the call speaks at 0–2 s, you
    /// at 2–3 s, and the mixed transcription made one line out of both. The old loudness guess
    /// over that line (mic ≥ 1.2× call to count as You) labelled your words as Call.
    #[test]
    fn turn_taking_chunk_keeps_your_words_as_you() {
        let sys = voice(4.0, 0.0, 2.0, 6000);
        let mic = voice(4.0, 2.0, 3.0, 6000);
        // what the removed audio::attribute would have decided for the mixed 0–3 s line
        let (a, b) = (0, 3 * SR);
        assert!(audio::rms(&mic[a..b]) < 1.2 * audio::rms(&sys[a..b]), "the old rule said Call for this line");

        let parts = with_speech(&mic, &sys);
        assert_eq!(parts.iter().map(|p| p.0).collect::<Vec<_>>(), vec![Track::You, Track::Call], "mic first, then call");
        let (you, call) = (fake_whisper("tudo bem?"), fake_whisper("E aí Vini,"));
        let mut sent = Vec::new();
        let out = transcribe_parts(&parts, 60.0, |s| {
            sent.push(s.len());
            Ok(if std::ptr::eq(s, mic.as_slice()) { you(s) } else { call(s) })
        });
        assert_eq!(sent.len(), 2);
        assert!(out.errors.is_empty());
        let lines: Vec<(Track, &str, f64)> = out.segments.iter().map(|s| (s.track, s.text.as_str(), s.start)).collect();
        assert_eq!(lines, vec![(Track::Call, "E aí Vini,", 60.0), (Track::You, "tudo bem?", 62.0)]);
        assert!(out.segments[1].words.iter().all(|w| w.start >= 62.0), "words move to meeting time too");
    }

    #[test]
    fn silent_track_is_not_sent() {
        let sys = voice(4.0, 0.0, 3.0, 6000);
        let quiet_mic: Vec<i16> = (0..4 * SR).map(|i| ((i * 7919) % 301) as i16 - 150).collect();
        let parts = with_speech(&quiet_mic, &sys);
        assert_eq!(parts.iter().map(|p| p.0).collect::<Vec<_>>(), vec![Track::Call]);
        assert!(with_speech(&quiet_mic, &[0; 4 * SR]).is_empty(), "nothing to send at all");
    }

    #[test]
    fn one_failed_track_keeps_the_other() {
        let (mic, sys) = (voice(2.0, 0.0, 1.0, 6000), voice(2.0, 1.0, 2.0, 6000));
        let parts = with_speech(&mic, &sys);
        let call = fake_whisper("oi");
        let out = transcribe_parts(&parts, 0.0, |s| if std::ptr::eq(s, mic.as_slice()) { anyhow::bail!("timeout") } else { Ok(call(s)) });
        assert_eq!(out.errors.len(), 1);
        assert_eq!(out.errors[0].0, Track::You);
        assert_eq!(out.segments.len(), 1);
        assert_eq!(out.segments[0].track, Track::Call);
    }

    #[test]
    fn merge_orders_both_tracks_by_start() {
        // a turn-taking chunk: the call speaks first, you answer, the call again
        let call = vec![line(Track::Call, 0.0, 2.0, "E aí Vini,"), line(Track::Call, 3.1, 5.0, "tudo certo por aí?")];
        let mic = vec![line(Track::You, 2.0, 3.0, "tudo bem?"), line(Track::You, 5.2, 6.0, "tudo")];
        let (merged, dropped) = merge(mic, call);
        assert_eq!(dropped, 0);
        let order: Vec<(Track, f64)> = merged.iter().map(|s| (s.track, s.start)).collect();
        assert_eq!(order, vec![(Track::Call, 0.0), (Track::You, 2.0), (Track::Call, 3.1), (Track::You, 5.2)]);
    }
}
