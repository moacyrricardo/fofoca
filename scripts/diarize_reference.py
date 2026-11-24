"""Reference CAM++ embedding for fofoca's golden-embedding test (spec 022).

Rebuilds the fixed 3 s input that `diarize::tests::embedding_matches_the_python_reference` builds,
runs it through Python onnxruntime + kaldi-native-fbank (80 bins, dither 0, Povey window,
mean-normalized, like `diarize::embed`) and writes the JSON the test compares against.

usage: python scripts/diarize_reference.py <wespeaker_en_voxceleb_CAM++.onnx> \
           [crates/fofoca-core/tests/data/campp_reference.json]
needs: pip install onnxruntime kaldi-native-fbank numpy
"""
import json
import math
import sys

import kaldi_native_fbank as knf
import numpy as np
import onnxruntime as ort

SR = 16000
SECONDS = 3
SEED = 22
DESCRIPTION = (
    "3 s at 16 kHz, int16 scale: floor(3000 sin(2pi 220 t) + 2000 sin(2pi 440 t + 0.5) "
    "+ 1000 sin(2pi 1234 t) + 2000 (u - 0.5) + 0.5), u from the LCG "
    "s = (s * 1103515245 + 12345) mod 2^31, u = s / 2^31, seed 22. Then /32768, "
    "80-bin Kaldi fbank (dither 0, Povey window, snip_edges), mean-normalized, CAM++ via "
    "onnxruntime with default graph optimization."
)


def test_input():
    """Must match `golden_input()` in crates/fofoca-core/src/diarize.rs."""
    s = SEED
    out = []
    for n in range(SR * SECONDS):
        t = n / SR
        s = (s * 1103515245 + 12345) % 2**31
        u = s / 2**31
        v = 3000 * math.sin(2 * math.pi * 220 * t) + 2000 * math.sin(2 * math.pi * 440 * t + 0.5) \
            + 1000 * math.sin(2 * math.pi * 1234 * t) + 2000 * (u - 0.5)
        out.append(max(-32768, min(32767, math.floor(v + 0.5))))
    return out


def embedding(model, samples):
    o = knf.FbankOptions()
    o.frame_opts.dither = 0
    o.frame_opts.samp_freq = SR
    o.frame_opts.window_type = "povey"
    o.frame_opts.snip_edges = True
    o.mel_opts.num_bins = 80
    f = knf.OnlineFbank(o)
    f.accept_waveform(SR, [x / 32768.0 for x in samples])
    f.input_finished()
    feats = np.stack([f.get_frame(i) for i in range(f.num_frames_ready)])
    feats -= feats.mean(0)
    sess = ort.InferenceSession(model)
    return sess.run(None, {"feats": feats[None].astype(np.float32)})[0][0]


def main():
    model = sys.argv[1]
    out = sys.argv[2] if len(sys.argv) > 2 else "crates/fofoca-core/tests/data/campp_reference.json"
    e = embedding(model, test_input())
    with open(out, "w") as f:
        json.dump({
            "description": DESCRIPTION,
            "onnxruntime": ort.__version__,
            "norm": float(np.linalg.norm(e)),
            "embedding": [round(float(x), 6) for x in e],
        }, f, indent=1)
        f.write("\n")
    print(f"{out}: {len(e)} floats, norm {np.linalg.norm(e):.4f}")


if __name__ == "__main__":
    main()
