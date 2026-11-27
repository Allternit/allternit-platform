/**
 * Prints the Gizzi Code header mark and the `--help` banner with the real
 * renderers, so you can check them in Apple Terminal:  bun script/mascot-preview.ts
 */
import { mascotRows } from "../src/cli/ui/ink-app/components/welcomeArt"
import { ansiArtRow, helpBannerLines } from "../src/cli/ui/logo"

const header = mascotRows("mid").map(ansiArtRow)
console.log("header mark (18 x 8 cells):")
console.log(header.join("\n"))
console.log("\n--help banner:")
console.log(helpBannerLines().join("\n"))
