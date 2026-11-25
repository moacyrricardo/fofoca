#!/usr/bin/env bash
# fo-foca setup: downloads the models and the two engines fo-foca expects, into the
# places its default config points at (~/.local/share/fofoca/{models,bin}).
#
#   ./fofoca-setup.sh               models + llama-server + build whisper-server
#   ./fofoca-setup.sh --small       smaller models (Whisper small, Qwen3-1.7B)
#   ./fofoca-setup.sh --models-only skip the engines
#
# Re-running is safe: files that are already complete are skipped.
set -euo pipefail

DATA="${XDG_DATA_HOME:-$HOME/.local/share}/fofoca"
MODELS="$DATA/models"
LLAMA_TAG="b12527"
WHISPER_TAG="v1.9.6"

small=0 engines=1
for a in "$@"; do
  case "$a" in
    --small) small=1 ;;
    --models-only) engines=0 ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

need() { command -v "$1" >/dev/null || { echo "missing: $1 ($2)" >&2; exit 1; }; }
need curl "sudo apt install curl"

# get <url> <dest> <exact size in bytes>
get() {
  local url=$1 dest=$2 size=$3
  if [[ -f $dest && $(stat -c %s "$dest") == "$size" ]]; then
    echo "  ok      $(basename "$dest")"; return
  fi
  echo "  fetch   $(basename "$dest") ($((size / 1048576)) MB)"
  curl -fL --retry 3 -C - -o "$dest.part" "$url"
  local got; got=$(stat -c %s "$dest.part")
  [[ $got == "$size" ]] || { echo "size mismatch for $dest: $got != $size" >&2; exit 1; }
  mv "$dest.part" "$dest"
}

HF=https://huggingface.co
PY=https://github.com/thewh1teagle/pyannote-rs/releases/download/v0.1.0
mkdir -p "$MODELS"
echo "Models → $MODELS"
if [[ $small == 1 ]]; then
  get "$HF/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin" "$MODELS/ggml-small-q5_1.bin" 190085487
  get "$HF/unsloth/Qwen3-1.7B-GGUF/resolve/main/Qwen3-1.7B-Q4_K_M.gguf" "$MODELS/Qwen3-1.7B-Q4_K_M.gguf" 1107409472
else
  get "$HF/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin" "$MODELS/ggml-large-v3-turbo-q5_0.bin" 574041195
  get "$HF/unsloth/Qwen3.5-2B-GGUF/resolve/main/Qwen3.5-2B-Q4_K_M.gguf" "$MODELS/Qwen3.5-2B-Q4_K_M.gguf" 1280835840
fi
get "$HF/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin" "$MODELS/ggml-silero-v5.1.2.bin" 885098
get "$PY/segmentation-3.0.onnx" "$MODELS/segmentation-3.0.onnx" 5983836
get "$PY/wespeaker_en_voxceleb_CAM++.onnx" "$MODELS/wespeaker_en_voxceleb_CAM++.onnx" 29292684

if [[ $engines == 1 ]]; then
  need tar "sudo apt install tar"
  echo "llama-server ($LLAMA_TAG, Vulkan) → $DATA/bin/llama"
  if [[ -x $DATA/bin/llama/llama-server ]]; then
    echo "  ok      llama-server"
  else
    tmp=$(mktemp -d)
    curl -fL --retry 3 -o "$tmp/llama.tar.gz" \
      "https://github.com/ggml-org/llama.cpp/releases/download/$LLAMA_TAG/llama-$LLAMA_TAG-bin-ubuntu-vulkan-x64.tar.gz"
    mkdir -p "$tmp/x" && tar -xzf "$tmp/llama.tar.gz" -C "$tmp/x"
    srv=$(find "$tmp/x" -name llama-server -type f | head -1)
    [[ -n $srv ]] || { echo "llama-server not found in the archive" >&2; exit 1; }
    mkdir -p "$DATA/bin/llama" && cp -a "$(dirname "$srv")"/. "$DATA/bin/llama/"
    rm -rf "$tmp"
  fi

  echo "whisper-server ($WHISPER_TAG, Vulkan, built from source) → $DATA/bin/whisper"
  if [[ -x $DATA/bin/whisper/whisper-server ]]; then
    echo "  ok      whisper-server"
  else
    need git "sudo apt install git"
    need cmake "sudo apt install cmake build-essential"
    need glslc "sudo apt install glslc libvulkan-dev"
    tmp=$(mktemp -d)
    git clone -q --depth 1 --branch "$WHISPER_TAG" https://github.com/ggml-org/whisper.cpp "$tmp/whisper.cpp"
    cmake -S "$tmp/whisper.cpp" -B "$tmp/build" -DCMAKE_BUILD_TYPE=Release \
      -DGGML_VULKAN=ON -DBUILD_SHARED_LIBS=ON -DWHISPER_BUILD_EXAMPLES=ON \
      -DCMAKE_INSTALL_RPATH='$ORIGIN' -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON >/dev/null
    cmake --build "$tmp/build" -j"$(nproc)" --target whisper-server
    mkdir -p "$DATA/bin/whisper"
    cp "$tmp/build/bin/whisper-server" "$DATA/bin/whisper/"
    find "$tmp/build" -name '*.so*' -exec cp -a {} "$DATA/bin/whisper/" \;
    rm -rf "$tmp"
  fi
fi

cat <<EOF

Done. Start fo-foca, then in Settings → Models check that every row is green.
EOF
if [[ $small == 1 ]]; then
  cat <<EOF
You used --small: in Settings → Models pick "Whisper small" and "Qwen3-1.7B"
(the default config points at the larger models).
EOF
fi
cat <<EOF
GPU: in Settings pick the Vulkan device for each engine (llama-server --list-devices shows them).
EOF
