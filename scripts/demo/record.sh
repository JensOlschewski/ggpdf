#!/usr/bin/env bash
# Records a scripted ggPdf session to a GIF (macOS).
# Usage: scripts/demo/record.sh [steps-file] [output.gif]
# Needs Accessibility + Screen Recording permission for the terminal, plus ffmpeg.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
steps="${1:-$root/scripts/demo/demo.steps}"
out="${2:-$root/docs/assets/demo.gif}"
pdf="${DEMO_PDF:-$root/assets/test-pdfs/letter.pdf}"
width=1000 height=700 x=100 y=80 fps=15 gif_width=800

command -v ffmpeg >/dev/null || { echo "ffmpeg not found" >&2; exit 1; }

work="$(mktemp -d)"
app_pid="" rec_pid=""
cleanup() {
  [[ -n "$rec_pid" ]] && kill -INT "$rec_pid" 2>/dev/null || true
  [[ -n "$app_pid" ]] && kill "$app_pid" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

sys() { osascript -e "tell application \"System Events\" to $1"; }

key_code() {
  case "$1" in
    esc) echo 53 ;; return) echo 36 ;; tab) echo 48 ;;
    *) echo "unknown key: $1" >&2; exit 1 ;;
  esac
}

press() { # press CHAR [modifier list e.g. "control down, shift down"]
  if [[ -n "${2:-}" ]]; then
    sys "keystroke \"$1\" using {$2}"
  else
    sys "keystroke \"$1\""
  fi
}

run_step() {
  local cmd="$1" arg="${2:-}"
  case "$cmd" in
    sleep) sleep "$arg" ;;
    type)
      for ((i = 0; i < ${#arg}; i++)); do press "${arg:i:1}"; sleep 0.12; done ;;
    key) sys "key code $(key_code "$arg")" ;;
    combo)
      local mods="" part
      IFS='+' read -ra parts <<<"$arg"
      for part in "${parts[@]::${#parts[@]}-1}"; do
        case "$part" in
          ctrl) part=control ;; alt) part=option ;; cmd) part=command ;;
        esac
        mods+="${mods:+, }$part down"
      done
      press "${parts[${#parts[@]}-1]}" "$mods" ;;
    *) echo "unknown step: $cmd" >&2; exit 1 ;;
  esac
}

echo "building..."
cargo build --release --manifest-path "$root/Cargo.toml" --quiet

# Empty config dir so the demo always runs with default settings and keys.
mkdir -p "$work/config" "$work/home"
XDG_CONFIG_HOME="$work/config" HOME="$work/home" PDFIUM_LIB_DIR="$root/pdfium" \
  "$root/target/release/ggpdf" "$pdf" >/dev/null 2>&1 &
app_pid=$!

for _ in {1..50}; do
  sys "exists (process 1 whose unix id is $app_pid)" 2>/dev/null | grep -q true && break
  sleep 0.2
done
sleep 1
sys "set frontmost of (first process whose unix id is $app_pid) to true"
sys "tell (first process whose unix id is $app_pid) to set {position, size} of window 1 to {{$x, $y}, {$width, $height}}"
sleep 1

screencapture -v -x -R "$x,$y,$width,$height" "$work/rec.mov" &
rec_pid=$!
sleep 1

while read -r cmd arg; do
  [[ -z "$cmd" || "$cmd" == \#* ]] && continue
  run_step "$cmd" "$arg"
done <"$steps"

kill -INT "$rec_pid"; wait "$rec_pid" 2>/dev/null || true
rec_pid=""

mkdir -p "$(dirname "$out")"
vf="fps=$fps,scale=$gif_width:-1:flags=lanczos"
ffmpeg -v error -y -i "$work/rec.mov" -vf "$vf,palettegen=stats_mode=diff" "$work/palette.png"
ffmpeg -v error -y -i "$work/rec.mov" -i "$work/palette.png" \
  -lavfi "$vf[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=5" "$out"
echo "wrote $out"
