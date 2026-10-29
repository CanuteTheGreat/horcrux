# Horcrux branding

`horcrux-logo-official.png` (1024x1024) is the **official Horcrux logo**, supplied by the owner on 2026-10-28.
Do not redraw or restyle it. Everything else here is derived from it by crop, scale and a dark rounded background
tile only (no AI upscaling, no redrawing); regenerate with `branding/derive.py` (see its docstring for the crop boxes
used: `CUTY=637 derive.py SRC OUT horcrux 150,-45,875,680 140,637,885,800 410,430,620,640`).

| file | what | used by |
|---|---|---|
| `horcrux-logo-official.png` | the original | source of truth, README header |
| `horcrux-mark-{512,256,180,128,64}.png` | the crest (shield, bird + serpent, gear + star, spider, swords, webs) without the wordmark, on a dark rounded tile | web navbar / top bar, apple-touch-icon (180), installer, boot menu |
| `horcrux-mark-{48,32,16}.png`, `favicon.ico` | tight gear + star crop (the full crest turns to mush below ~48 px) | favicon |
| `horcrux-wordmark.png` | the `HORCRUX` text only | wide headers |
| `horcrux-full-half.png` | the whole logo at 512x512 | login page, boot splash, installer welcome |

The web UI ships the web-sized subset from `horcrux-api/horcrux-ui/assets/brand/` (copies of these files).
| `horcrux-grub-splash.png` | 1024x768 GRUB background: the full logo scaled and centred under the menu (compose with the full logo only) | live-ISO boot menu |
