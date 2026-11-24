//! Speaker separation after the call (spec 003), on CPU.
//!
//! ```text
//! system.wav ─▶ segmentation-3.0 (10 s windows) ─▶ speech turns ─▶ WeSpeaker CAM++ embedding
//!            ─▶ average-linkage clustering (cosine) ─▶ Turn { start, end, speaker }
//! ```
//!
//! The ONNX handling follows pyannote-rs (MIT, github.com/thewh1teagle/pyannote-rs), rebuilt on
//! `ort` + the pure-Rust `kaldi-fbank` so no C++ toolchain is needed. Two changes from it: turns
//! are also split where the segmentation's speaker class changes (not only at silences), and
//! clustering runs over all turns at the end instead of online, since the whole call is available.

use anyhow::{bail, Context, Result};
use ndarray::{Array1, Array2, Axis};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::{Tensor, TensorRef};
use std::path::Path;

use crate::audio::SAMPLE_RATE;

/// A stretch of one speaker's voice, in seconds from the start of the audio.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    /// 1-based speaker number, ordered by first appearance.
    pub speaker: u32,
}

pub struct Options {
    /// Cosine similarity above which two clusters are the same person. Agglomeration stops here.
    pub threshold: f32,
    /// Cap on speakers, applied only after small clusters are folded in. `None` = no cap.
    pub max_speakers: Option<usize>,
    /// A cluster with less speech than `min(min_cluster_s, 10 % of all embedded speech)` is folded
    /// into the nearest larger one. Normally [`MIN_CLUSTER_S`].
    pub min_cluster_s: f64,
}

/// Default minimum speech (seconds) for a cluster to stand as its own speaker (spec 022).
pub const MIN_CLUSTER_S: f64 = 10.0;

/// Turns shorter than this get no embedding of their own (too little voice to identify); they
/// take the speaker of the nearest identified turn.
const MIN_EMBED_S: f64 = 0.8;
/// Class changes that last less than this are merged into the surrounding turn.
const MIN_TURN_S: f64 = 0.4;

pub fn separate(samples: &[i16], segmentation_model: &Path, embedding_model: &Path, opts: &Options) -> Result<Vec<Turn>> {
    let regions = speech_regions(samples, segmentation_model)?;
    if regions.is_empty() {
        return Ok(Vec::new());
    }
    // Graph optimization OFF for CAM++ (spec 022): the ONNX Runtime bundled with ort 2.0.0-rc.10
    // corrupts this model at any level ≥ Level1 (norm ~150 vectors, cosine ≈ 0 to the right
    // embedding). Re-run the golden-embedding test before re-enabling it after an `ort` upgrade.
    let mut embedder = session(embedding_model, GraphOptimizationLevel::Disable)?;
    let mut embeddings: Vec<Option<Vec<f32>>> = Vec::with_capacity(regions.len());
    for &(start, end) in &regions {
        let e = if end - start >= MIN_EMBED_S {
            let a = (start * SAMPLE_RATE as f64) as usize;
            let b = ((end * SAMPLE_RATE as f64) as usize).min(samples.len());
            Some(embed(&mut embedder, &samples[a..b])?)
        } else {
            None
        };
        embeddings.push(e);
    }
    let durations: Vec<f64> = regions.iter().map(|&(start, end)| end - start).collect();
    let labels = cluster(&embeddings, &durations, opts);
    Ok(regions.iter().zip(labels).map(|(&(start, end), speaker)| Turn { start, end, speaker }).collect())
}

fn session(path: &Path, optimization: GraphOptimizationLevel) -> Result<Session> {
    if !path.exists() {
        bail!("model not found: {}", path.display());
    }
    Session::builder()?
        .with_optimization_level(optimization)?
        .with_intra_threads(4)?
        .commit_from_file(path)
        .with_context(|| format!("loading {}", path.display()))
}

// segmentation-3.0 framing, as in pyannote-rs: 10 s windows, first frame at sample 721, one frame
// per 270 samples; output = 7 powerset classes per frame (0 = nobody speaks).
const SEG_WINDOW: usize = SAMPLE_RATE as usize * 10;
const SEG_FIRST: usize = 721;
const SEG_FRAME: usize = 270;

/// Start of frame `frame` of segmentation window `window`, in seconds. Frames restart at each
/// window: a window holds 589 frames × 270 = 159 030 samples, not 160 000, so numbering them
/// globally drifts 60.6 ms per window (spec 022).
fn frame_time(window: usize, frame: usize) -> f64 {
    (window * SEG_WINDOW + SEG_FIRST + frame * SEG_FRAME) as f64 / SAMPLE_RATE as f64
}

/// Speech regions `(start_s, end_s)` from segmentation-3.0, split at speaker-class changes.
fn speech_regions(samples: &[i16], model: &Path) -> Result<Vec<(f64, f64)>> {
    // segmentation-3.0 is unaffected by the optimizer (identical regions either way).
    let mut sess = session(model, GraphOptimizationLevel::Level3)?;
    let mut padded = samples.to_vec();
    padded.resize(samples.len().div_ceil(SEG_WINDOW).max(1) * SEG_WINDOW, 0);

    let mut classes: Vec<usize> = Vec::new();
    let mut positions: Vec<(usize, usize)> = Vec::new(); // (window, frame in window) per class
    for (w, chunk) in padded.chunks(SEG_WINDOW).enumerate() {
        let input = Array1::from_iter(chunk.iter().map(|&x| x as f32)).insert_axis(Axis(0)).insert_axis(Axis(0));
        let out = sess.run(ort::inputs![TensorRef::from_array_view(input.view().into_dyn())?])?;
        let (shape, data) = out.get("output").context("segmentation output")?.try_extract_tensor::<f32>()?;
        let n_classes = *shape.last().context("segmentation shape")? as usize;
        for (f, frame) in data.chunks(n_classes).enumerate() {
            let best = frame.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i).unwrap_or(0);
            classes.push(best);
            positions.push((w, f));
        }
    }
    let audio_s = samples.len() as f64 / SAMPLE_RATE as f64;
    // `regions_from_classes` asks for one past the last frame as an end time
    let time = |i: usize| {
        let (w, f) = positions.get(i).copied().unwrap_or_else(|| positions.last().map_or((0, 0), |&(w, f)| (w, f + 1)));
        frame_time(w, f).min(audio_s)
    };
    Ok(regions_from_classes(&classes, time))
}

/// Group per-frame classes into regions: runs of the same non-zero class; short runs are merged
/// into the previous region when they touch it.
fn regions_from_classes(classes: &[usize], time: impl Fn(usize) -> f64) -> Vec<(f64, f64)> {
    let mut runs: Vec<(usize, usize, usize)> = Vec::new(); // (class, first frame, end frame)
    for (i, &c) in classes.iter().enumerate() {
        match runs.last_mut() {
            Some(r) if r.0 == c && r.2 == i => r.2 = i + 1,
            _ => runs.push((c, i, i + 1)),
        }
    }
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut prev_end_frame = usize::MAX;
    for (c, a, b) in runs {
        if c == 0 {
            continue;
        }
        let (start, end) = (time(a), time(b));
        let touches = prev_end_frame == a;
        match out.last_mut() {
            Some(last) if touches && (end - start < MIN_TURN_S || last.1 - last.0 < MIN_TURN_S) => last.1 = end,
            _ => out.push((start, end)),
        }
        prev_end_frame = b;
    }
    out.retain(|(s, e)| e > s);
    out
}

/// WeSpeaker embedding of one region: 80-bin Kaldi fbank (no dither), mean-normalized.
fn embed(sess: &mut Session, samples: &[i16]) -> Result<Vec<f32>> {
    let wave: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
    let mut opts = kaldi_fbank::FbankOptions::default();
    opts.frame_opts.dither = 0.0;
    opts.frame_opts.samp_freq = SAMPLE_RATE as f32;
    opts.frame_opts.snip_edges = true;
    opts.mel_opts.num_bins = 80;
    let frames = kaldi_fbank::compute_fbank(&opts, &wave);
    if frames.is_empty() {
        bail!("region too short for features");
    }
    let mut feats = Array2::from_shape_vec((frames.len(), 80), frames.concat())?;
    let mean = feats.mean_axis(Axis(0)).context("fbank mean")?;
    feats -= &mean;
    let out = sess.run(ort::inputs!["feats" => Tensor::from_array(feats.insert_axis(Axis(0)))?])?;
    let (_, data) = out.get("embs").context("embedding output")?.try_extract_tensor::<f32>()?;
    Ok(data.to_vec())
}

fn normalize(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter().map(|x| x / n).collect()
}

fn sim(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The most similar pair `(i, j, similarity)` with `i < j` under `score`; `None` if fewer than 2.
fn most_similar_pair(n: usize, score: impl Fn(usize, usize) -> f32) -> Option<(usize, usize, f32)> {
    let mut best: Option<(usize, usize, f32)> = None;
    for i in 0..n {
        for j in i + 1..n {
            let s = score(i, j);
            if best.is_none_or(|b| s > b.2) {
                best = Some((i, j, s));
            }
        }
    }
    best
}

/// Average-linkage agglomerative clustering on cosine similarity, stopping at the threshold; then
/// small clusters are folded into the nearest large one, then the optional speaker cap merges
/// large clusters (spec 022). `durations` are the regions' lengths in seconds. Regions without an
/// embedding take the label of the nearest region (by index, i.e. time) that has one. Labels are
/// 1-based in order of first appearance.
fn cluster(embeddings: &[Option<Vec<f32>>], durations: &[f64], opts: &Options) -> Vec<u32> {
    let items: Vec<(usize, Vec<f32>)> = embeddings.iter().enumerate().filter_map(|(i, e)| e.as_ref().map(|e| (i, normalize(e)))).collect();
    if items.is_empty() {
        return vec![1; embeddings.len()];
    }
    // clusters as lists of item indices; pairwise average similarity recomputed (n is small)
    let mut clusters: Vec<Vec<usize>> = (0..items.len()).map(|i| vec![i]).collect();
    let avg = |a: &[usize], b: &[usize]| {
        let mut s = 0.0;
        for &i in a {
            for &j in b {
                s += sim(&items[i].1, &items[j].1);
            }
        }
        s / (a.len() * b.len()) as f32
    };
    // no forced merges here: a cap applied during agglomeration glues real speakers together
    // while short noisy clusters stay apart
    while let Some((i, j, s)) = most_similar_pair(clusters.len(), |i, j| avg(&clusters[i], &clusters[j])) {
        if s < opts.threshold {
            break;
        }
        let merged = clusters.remove(j);
        clusters[i].extend(merged);
    }

    // minimum cluster size: fold each region of a small cluster into the most similar large one
    let speech = |c: &[usize]| c.iter().map(|&m| durations[items[m].0]).sum::<f64>();
    let total: f64 = items.iter().map(|(i, _)| durations[*i]).sum();
    let min_s = opts.min_cluster_s.min(0.1 * total);
    let (mut large, mut small): (Vec<Vec<usize>>, Vec<Vec<usize>>) = clusters.into_iter().partition(|c| speech(c) >= min_s);
    if large.is_empty() {
        let k = (0..small.len()).max_by(|&a, &b| speech(&small[a]).total_cmp(&speech(&small[b]))).unwrap_or(0);
        large.push(small.remove(k));
    }
    let centroid = |c: &[usize]| {
        let mut m = vec![0.0f32; items[c[0]].1.len()];
        for &i in c {
            for (x, y) in m.iter_mut().zip(&items[i].1) {
                *x += y;
            }
        }
        normalize(&m)
    };
    let centroids: Vec<Vec<f32>> = large.iter().map(|c| centroid(c)).collect();
    for m in small.into_iter().flatten() {
        let k = (0..large.len()).max_by(|&a, &b| sim(&items[m].1, &centroids[a]).total_cmp(&sim(&items[m].1, &centroids[b]))).unwrap_or(0);
        large[k].push(m);
    }

    // speaker cap, after cleanup only
    if let Some(n) = opts.max_speakers {
        while large.len() > n.max(1) {
            let cs: Vec<Vec<f32>> = large.iter().map(|c| centroid(c)).collect();
            let Some((i, j, _)) = most_similar_pair(large.len(), |i, j| sim(&cs[i], &cs[j])) else { break };
            let merged = large.remove(j);
            large[i].extend(merged);
        }
    }
    let clusters = large;
    let mut label_of_item = vec![0usize; items.len()];
    for (c, members) in clusters.iter().enumerate() {
        for &m in members {
            label_of_item[m] = c;
        }
    }
    // per region: own cluster, or nearest embedded neighbour's
    let mut raw = vec![0usize; embeddings.len()];
    for (r, slot) in raw.iter_mut().enumerate() {
        let nearest = items.iter().enumerate().min_by_key(|(_, (i, _))| i.abs_diff(r)).map(|(k, _)| k).unwrap_or(0);
        *slot = label_of_item[nearest];
    }
    // renumber by first appearance
    let mut order: Vec<usize> = Vec::new();
    raw.iter()
        .map(|c| {
            let pos = order.iter().position(|x| x == c).unwrap_or_else(|| {
                order.push(*c);
                order.len() - 1
            });
            pos as u32 + 1
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_split_on_class_change_and_merge_blips() {
        // 10 frames per second for readability
        let t = |f: usize| f as f64 / 10.0;
        // silence, speaker A 2 s, B 1.5 s, a 0.2 s blip of C, B again 1 s, silence, A 1 s
        let mut c = vec![0; 5];
        c.extend([1; 20]);
        c.extend([2; 15]);
        c.extend([3; 2]);
        c.extend([2; 10]);
        c.extend([0; 5]);
        c.extend([1; 10]);
        let r = regions_from_classes(&c, t);
        // the 0.2 s blip is merged into the previous turn
        assert_eq!(r.len(), 4, "{r:?}");
        assert_eq!(r[0], (0.5, 2.5));
        assert_eq!(r[1].0, 2.5);
        assert!((r[1].1 - 4.2).abs() < 1e-9, "blip merged: {r:?}");
        assert_eq!(r[3], (5.7, 6.7));
    }

    #[test]
    fn frame_time_restarts_at_each_window() {
        assert_eq!(frame_time(0, 0), 721.0 / 16000.0);
        assert_eq!(frame_time(3, 0), 30.0 + 721.0 / 16000.0);
        // regression: global numbering put window 3 frame 0 at frame 3×589, 181.8 ms early
        assert!(frame_time(3, 0) - (721 + 3 * 589 * 270) as f64 / 16000.0 > 0.18);
        // never runs backwards across a window boundary
        for w in 0..10 {
            assert!(frame_time(w + 1, 0) > frame_time(w, 588));
            assert!(frame_time(w, 1) > frame_time(w, 0));
        }
    }

    #[test]
    fn clustering_groups_similar_voices() {
        let a = Some(vec![1.0, 0.1, 0.0]);
        let a2 = Some(vec![0.9, 0.2, 0.0]);
        let b = Some(vec![0.0, 1.0, 0.1]);
        let b2 = Some(vec![0.1, 0.9, 0.0]);
        let opts = Options { threshold: 0.5, max_speakers: None, min_cluster_s: MIN_CLUSTER_S };
        // a short region without embedding takes its nearest neighbour's label
        assert_eq!(cluster(&[a.clone(), b.clone(), None, b2.clone(), a2.clone()], &[5.0, 5.0, 0.5, 5.0, 5.0], &opts), vec![1, 2, 2, 2, 1]);
        // nothing embedded: everyone is speaker 1
        assert_eq!(cluster(&[None, None], &[0.5, 0.5], &opts), vec![1, 1]);
    }

    fn speakers(labels: &[u32]) -> usize {
        labels.iter().collect::<std::collections::HashSet<_>>().len()
    }

    #[test]
    fn small_clusters_fold_into_the_nearest_large_one() {
        let a = Some(vec![1.0, 0.1, 0.0]);
        let a2 = Some(vec![0.9, 0.2, 0.0]);
        let b = Some(vec![0.0, 1.0, 0.1]);
        let b2 = Some(vec![0.1, 0.9, 0.0]);
        // far from both groups (below the threshold), but nearer to A
        let odd = Some(vec![0.3, 0.0, 1.0]);
        let regions = [a, b, odd, b2, a2];
        let durations = [8.0, 8.0, 1.0, 8.0, 8.0];
        let opts = Options { threshold: 0.4, max_speakers: None, min_cluster_s: MIN_CLUSTER_S };
        assert_eq!(cluster(&regions, &durations, &opts), vec![1, 2, 1, 2, 1]);
        // the cap applies after cleanup
        let capped = Options { max_speakers: Some(1), ..opts };
        assert_eq!(cluster(&regions, &durations, &capped), vec![1; 5]);
    }

    #[test]
    fn no_forced_merges_without_a_cap() {
        // regression: the old max_speakers forced merges during agglomeration
        let v = |x: f32, y: f32, z: f32| Some(vec![x, y, z]);
        let regions = [v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0), v(0.0, 0.0, 1.0), v(0.7, 0.7, 0.0)];
        let opts = Options { threshold: 1.01, max_speakers: None, min_cluster_s: 0.0 };
        assert_eq!(cluster(&regions, &[5.0; 4], &opts), vec![1, 2, 3, 4]);
        // with a cap of 2, only centroids merge: the two most similar first
        let opts2 = Options { max_speakers: Some(2), ..opts };
        assert_eq!(speakers(&cluster(&regions, &[5.0; 4], &opts2)), 2);
    }

    #[test]
    fn when_no_cluster_is_large_the_biggest_one_keeps_everyone() {
        // 12 orthogonal voices of 1 s each: every cluster holds 1/12 < 10 % of the speech
        let regions: Vec<Option<Vec<f32>>> = (0..12).map(|i| Some((0..12).map(|j| if i == j { 1.0 } else { 0.0 }).collect())).collect();
        let mut durations = vec![1.0; 12];
        durations[5] = 1.05;
        let opts = Options { threshold: 0.4, max_speakers: None, min_cluster_s: MIN_CLUSTER_S };
        assert_eq!(cluster(&regions, &durations, &opts), vec![1; 12]);
    }
}
