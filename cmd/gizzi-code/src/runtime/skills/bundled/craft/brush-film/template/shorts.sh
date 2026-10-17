#!/bin/bash
# Render and encode vertical Shorts listed in anim/src/shorts.js.  ./shorts.sh [name ...]   (no args = all)
set -e; cd "$(dirname "$0")"; NAME=$(cat .film-name 2>/dev/null || basename "$PWD"); mkdir -p exports/shorts
[ -f anim/assets/mix.wav ] || python3 audio/build_mix.py
LIST=$(node -e "const fs=require('fs');const s=fs.readFileSync('anim/src/shorts.js','utf8');const m=s.match(/const SHORTS = (\[[\s\S]*?\n\]);/);const SHORTS=eval(m[1]);for(const l of s.split('\n'))if(l.startsWith('SHORTS.push('))eval(l);const S=SHORTS;for(const x of S)console.log(x.name+' '+x.start+' '+x.end)")
echo "$LIST" | while read n a b; do
  [ -z "$n" ] && continue; [ $# -gt 0 ] && [[ ! " $* " == *" $n "* ]] && continue
  echo "== $n ($a–$b s)"; D=out/shorts/$n
  (cd anim && node render.mjs --frames --range=$a:$b --workers=${WORKERS:-4} --dir=$D --qs="ow=1080&oh=1920&short=$n" < /dev/null)
  F=$(ls anim/$D | head -1); START=$(( 10#$(echo $F | tr -dc 0-9) ))
  ffmpeg -nostdin -v error -y -framerate 24 -start_number $START -i anim/$D/f%05d.jpg -ss $a -t $(python3 -c "print($b-$a)") -i anim/assets/mix.wav -map 0:v -map 1:a \
    -c:v libx264 -preset medium -crf 19 -pix_fmt yuv420p -c:a aac -b:a 192k -af "afade=t=in:d=0.2,afade=t=out:st=$(python3 -c "print($b-$a-0.6)"):d=0.6,loudnorm=I=-14:TP=-1.5" -shortest -movflags +faststart exports/shorts/$NAME-$n.mp4
  ls -la exports/shorts/$NAME-$n.mp4
done
