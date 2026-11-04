#!/usr/bin/env bash

# Records a scripted ggPdf session to a GIF (macOS).
# Usage: scripts/demo/record.sh [steps-file] [output.gif]
# Needs Accessibility + Screen Recording permission for the terminal, plus ffmpeg.
# With AeroSpace, the demo window is made floating so it keeps the size below.
# With JankyBorders, ggPdf gets no border while recording.
# With KeyCastr installed, keystrokes are shown in the recording. Place its
# bezel inside the capture area below; KEYCASTR=0 records without it.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
steps="${1:-$root/scripts/demo/handling.steps}"
out="${2:-$root/docs/assets/ggpdf-handling.gif}"
pdf="${DEMO_PDF:-$root/assets/test-pdfs/letter.pdf}"

# Size of a single full-screen AeroSpace tile on a 1280x800 pt display
# (15 pt side/bottom gaps, 50 pt top gap), so the demo looks like real use.
# GIF frame delays are whole 1/100 s, so 25 or 50 fps play at the exact speed.
x=15 y=50 width=1250 height=734 fps="${FPS:-25}" gif_width=800

command -v ffmpeg >/dev/null || { echo "ffmpeg not found" >&2; exit 1; }

work="$(mktemp -d)"
app_pid="" rec_pid="" borders_off="" keycastr_started=""
keycastr=io.github.keycastr
cleanup() {
  [[ -n "$rec_pid" ]] && kill -INT "$rec_pid" 2>/dev/null || true
  [[ -n "$app_pid" ]] && kill "$app_pid" 2>/dev/null || true
  [[ -n "$borders_off" ]] && borders blacklist="" || true
  [[ -n "$keycastr_started" ]] && osascript -e "tell application id \"$keycastr\" to quit" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

# Turns the steps file into one AppleScript, so all keys go out from a
# single osascript run instead of one slow process per key.
# A "move X Y" step moves the mouse to X,Y pt from the window's top-left.
# Each caption step logs a wall-clock timestamp to $work/stamps and
# writes its text to $work/caption<N>.txt for drawtext below.
quote() { local s="${1//\\/\\\\}"; printf '"%s"' "${s//\"/\\\"}"; }
now="perl -MTime::HiRes=time -e 'printf qq(%.3f\\n), time'"
captions=0
steps_script() {
  echo 'tell application "System Events"'
  local cmd arg
  while read -r cmd arg; do
    [[ -z "$cmd" || "$cmd" == \#* ]] && continue
    case "$cmd" in
      sleep) echo "delay $arg" ;;
      type)
        for ((i = 0; i < ${#arg}; i++)); do
          echo "keystroke $(quote "${arg:i:1}")"
          echo "delay 0.12"
        done ;;
      key)
        case "$arg" in
          esc) echo "key code 53" ;; return) echo "key code 36" ;; tab) echo "key code 48" ;;
          *) echo "unknown key: $arg" >&2; exit 1 ;;
        esac ;;
      combo)
        local mods="" part parts
        IFS='+' read -ra parts <<<"$arg"
        for part in "${parts[@]::${#parts[@]}-1}"; do
          case "$part" in ctrl) part=control ;; alt) part=option ;; cmd) part=command ;; esac
          mods+="${mods:+, }$part down"
        done
        echo "keystroke $(quote "${parts[${#parts[@]}-1]}") using {$mods}" ;;
      move)
        # move X Y: put the mouse at X,Y pt inside the recorded window.
        read -r mx my <<<"$arg"
        js="ObjC.import(\"CoreGraphics\"); \$.CGEventPost(0, \$.CGEventCreateMouseEvent(\$(), 5, \$.CGPointMake($((x + mx)), $((y + my))), 0))"
        echo "do shell script $(quote "osascript -l JavaScript -e '$js'")" ;;
      caption)
        printf '%s' "$arg" | fold -s -w 40 >"$work/caption$captions.txt"
        echo "do shell script $(quote "$now >>'$work/stamps'")"
        captions=$((captions + 1)) ;;
      *) echo "unknown step: $cmd" >&2; exit 1 ;;
    esac
  done <"$steps"
  echo 'end tell'
}
steps_script >"$work/steps.applescript"

# Only slow after source changes (a full release build).
# echo "building..."
# cargo build --release --manifest-path "$root/Cargo.toml" --quiet

# The bundled app is "ggPdf", the cargo binary "ggpdf".
if pgrep -x borders >/dev/null; then borders blacklist="ggpdf,ggPdf"; borders_off=1; fi

# KeyCastr has no CLI; start it in the background and quit it afterwards,
# unless it was already running.
if [[ "${KEYCASTR:-1}" != 0 ]] && open -Ra KeyCastr 2>/dev/null && ! pgrep -x KeyCastr >/dev/null; then
  open -g -b "$keycastr"
  keycastr_started=1
fi

# Fresh config dir so the demo runs with default settings and keys. Only the
# finder dirs are set, so ctrl+f lists the example PDFs.
mkdir -p "$work/config/ggpdf" "$work/home"
printf '[finder]\ndirs = ["%s", "%s"]\nmax_depth = 1\n' "$root/assets" "$root/assets/test-pdfs" >"$work/config/ggpdf/config.toml"
XDG_CONFIG_HOME="$work/config" HOME="$work/home" PDFIUM_LIB_DIR="$root/pdfium" \
  "$root/target/release/ggpdf" "$pdf" >/dev/null 2>&1 &
app_pid=$!

if command -v aerospace >/dev/null; then
  window=""
  for _ in {1..50}; do
    window="$(aerospace list-windows --all --format '%{window-id} %{app-pid}' |
      awk -v pid="$app_pid" '$2 == pid { print $1; exit }')"
    [[ -n "$window" ]] && break
    sleep 0.2
  done
  [[ -n "$window" ]] || { echo "ggPdf window not found" >&2; exit 1; }
  aerospace layout --window-id "$window" floating
  aerospace focus --window-id "$window"
else
  sleep 2
fi
osascript -e "tell application \"System Events\" to tell (first process whose unix id is $app_pid)
  set frontmost to true
  set position of window 1 to {$x, $y}
  set size of window 1 to {$width, $height}
end tell"
sleep 0.5

screencapture -v -x -C -R "$x,$y,$width,$height" "$work/rec.mov" &
rec_pid=$!
sleep 1

osascript "$work/steps.applescript"

stop="$(sh -c "$now")"
kill -INT "$rec_pid"; wait "$rec_pid" 2>/dev/null || true
rec_pid=""

# Caption times relative to the video: the recording ends at $stop, so it
# started at $stop minus its duration. Each caption lasts until the next one.
draw=""
if ((captions > 0)); then
  duration="$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$work/rec.mov")"
  # shellcheck disable=SC2207
  times=($(awk -v t0="$stop" -v d="$duration" '{ printf "%.3f ", $1 - (t0 - d) } END { print d }' "$work/stamps"))
  for ((i = 0; i < captions; i++)); do
    [[ -s "$work/caption$i.txt" ]] || continue
    draw+="drawtext=fontfile=/System/Library/Fonts/SFNS.ttf:textfile=$work/caption$i.txt:expansion=none"
    draw+=":fontsize=h/15:fontcolor=white:box=1:boxcolor=black@0.6:boxborderw=24"
    draw+=":x=(w-text_w)/2:y=h-text_h-h/20:enable='between(t,${times[i]},${times[i + 1]})',"
  done
fi

mkdir -p "$(dirname "$out")"

# create gif
ffmpeg -v error -y -i "$work/rec.mov" -lavfi \
  "${draw}fps=$fps,scale=$gif_width:-1:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" \
  "$out"
echo "wrote $out"
