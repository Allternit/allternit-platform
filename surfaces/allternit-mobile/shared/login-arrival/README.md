# Offline login illustration

Approved 24-second loop: all95 original wordmark rectangles assemble; Gizzi arrives; the website cast joins; other bots leave; Gizzi tosses/catches a spark and winks; the scene dissolves and repeats.

`scene.js` retains the reviewed prototype's deterministic canvas drawing and the Allternit.com stage2d/cast2d rigs (Allternit Websites/projects/www.allternit.com/source/app/src/lib/cast), plus the approved wordmark and11 bot art assets embedded as data URLs. These are existing Allternit brand assets. The private reference recording is not included.

`player.js` owns lifecycle only. It caps painting at30fps, pauses offscreen/hidden and on parent/native pause, and shows a settled scene for Reduce Motion. No authentication data enters this document. Its hash-locked CSP forbids network requests.

Run `python3 build.py --web /path/to/allternit-ai` to emit identical offline HTML for the web public directory and the iOS resource. The generated HTML is committed so releases do not require a cross-repository build step. Verify SHA256 parity before merging. iOS loads only its bundle copy; the web iframe uses its local public asset.
