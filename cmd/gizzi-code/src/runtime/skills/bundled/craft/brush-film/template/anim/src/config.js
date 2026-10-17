// config.js: project settings.
//   duration: the video's length in seconds (= SHOT_T.end once your film has a timeline in film.js).
//   bpm:      the rhythm that idles, bounces and pulse() follow. Characters always move to some beat; if the video has
//             music, set this to the song's tempo and offset to the time in seconds of its first downbeat (audio/beat.py).
//   audio:    the mixed soundtrack, muxed in by render.mjs --clip/--encode when the file exists (audio/build_mix.py writes it).
const PROJECT = { duration: 7.2, bpm: 100, offset: 0, audio: "assets/mix.wav" };
