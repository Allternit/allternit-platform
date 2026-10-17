"""Score for this reel. Every cue time below should match a moment in scenes.js (hits, cuts, reveals).
score_lib.py is copied next to this file by new_project.sh."""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import score_lib as S
from score_lib import place, init, impact, blip, riser, whoosh, glitch, pad, tt, np

DUR = 8.0
init(DUR)
t = tt(2.0)
place('dry', 0.0, (np.sin(2 * np.pi * 55 * t) * 0.5) * np.minimum(1, t / 0.8) * 0.45)   # intro drone
for k in range(13):                                                                    # block landings
    place('dry', 0.55 + k * 0.045, blip([880, 1046.5, 1318.5][k % 3], 0.06, 0.12), pan=-0.6 + k * 0.1)
place('dry', 1.0, riser(1.0, 250, 7000, 0.55))                                         # into the drop
S.typing(1.2, 1.65, 12)
place('dry', 1.8, glitch(0.2))
place('dry', 2.0, impact(0.8)); place('wet', 2.0, impact(0.6))                         # DROP = first cut
place('dry', 2.5, impact(0.35))                                                        # second slam
S.groove(2.5, 6.5, bpm=120, roots={2.0: 55.0, 4.0: 49.0})
place('dry', 3.7, whoosh(0.45, 0.45))
place('dry', 4.0, impact(0.7))                                                         # wipe into product
for k in range(3): place('dry', 5.6 + k * 0.2, blip(1568, 0.05, 0.1))                 # rows flip to PASSED
place('dry', 6.2, whoosh(0.35, 0.4))
place('dry', 6.5, impact(1.0)); place('dry', 6.5, pad([220, 261.63, 329.63, 493.88], 1.5, 1800, 0.35))
S.write_mix('score.wav')
