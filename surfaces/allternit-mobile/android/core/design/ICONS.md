# Android icon set (Phosphor, mapped from iOS)

iOS draws its icons with SF Symbols, which Apple's licence keeps off Android. Desktop
(allternit-ai) uses `@phosphor-icons/react` and iOS ships the same family in
`Assets.xcassets/PhIcons`. Android uses one set: **Phosphor, regular weight** (fill
where iOS uses a `.fill` symbol), MIT licensed, generated from the official
`@phosphor-icons/react@2.1.10` path data into `core/design/src/main/res/drawable/ph_*.xml`.

Use `com.allternit.core.design.R.drawable.ph_<name>` everywhere. A feature module never
draws its own glyph: the same concept uses the same icon on every screen.

## Regenerate

```sh
node core/design/tools/gen-icons.mjs <allternit-ai>/node_modules/@phosphor-icons/react surfaces/allternit-mobile/android sf.txt
```

`sf.txt` is `grep -rhoE '(systemName|systemImage): *"[^"]+"' ios/Features ios/Shared`, de-duplicated.
The generator is `core/design/tools/gen-icons.mjs`.

## SF Symbol to Phosphor (189 symbols used in ios/Features)

| SF Symbol (iOS) | Phosphor | Android drawable |
|---|---|---|
| `antenna.radiowaves.left.and.right` | `broadcast` | `ph_broadcast` |
| `app.window` | `app-window` | `ph_app_window` |
| `apple.terminal` | `terminal-window` | `ph_terminal_window` |
| `archivebox` | `archive` | `ph_archive` |
| `arrow.clockwise` | `arrow-clockwise` | `ph_arrow_clockwise` |
| `arrow.counterclockwise` | `arrow-counter-clockwise` | `ph_arrow_counter_clockwise` |
| `arrow.down` | `arrow-down` | `ph_arrow_down` |
| `arrow.down.circle` | `arrow-circle-down` | `ph_arrow_circle_down` |
| `arrow.down.right.and.arrow.up.left` | `arrows-in` | `ph_arrows_in` |
| `arrow.left` | `arrow-left` | `ph_arrow_left` |
| `arrow.left.arrow.right` | `arrows-left-right` | `ph_arrows_left_right` |
| `arrow.right` | `arrow-right` | `ph_arrow_right` |
| `arrow.triangle.2.circlepath` | `arrows-clockwise` | `ph_arrows_clockwise` |
| `arrow.triangle.branch` | `git-branch` | `ph_git_branch` |
| `arrow.up` | `arrow-up` | `ph_arrow_up` |
| `arrow.up.circle.fill` | `arrow-circle-up-fill` | `ph_arrow_circle_up_fill` |
| `arrow.up.forward.app` | `arrow-square-out` | `ph_arrow_square_out` |
| `arrow.up.left.and.arrow.down.right` | `arrows-out` | `ph_arrows_out` |
| `arrow.up.right` | `arrow-up-right` | `ph_arrow_up_right` |
| `arrow.up.right.square` | `arrow-square-out` | `ph_arrow_square_out` |
| `arrow.up.to.line` | `arrow-line-up` | `ph_arrow_line_up` |
| `arrow.uturn.left` | `arrow-u-up-left` | `ph_arrow_u_up_left` |
| `at` | `at` | `ph_at` |
| `bolt` | `lightning` | `ph_lightning` |
| `bolt.fill` | `lightning-fill` | `ph_lightning_fill` |
| `bolt.slash` | `lightning-slash` | `ph_lightning_slash` |
| `bolt.trianglebadge.exclamationmark` | `lightning-a` | `ph_lightning_a` |
| `brain` | `brain` | `ph_brain` |
| `bubble.left` | `chat-circle` | `ph_chat_circle` |
| `bubble.left.and.bubble.right` | `chats-circle` | `ph_chats_circle` |
| `building.2` | `buildings` | `ph_buildings` |
| `calendar` | `calendar-blank` | `ph_calendar_blank` |
| `calendar.badge.checkmark` | `calendar-check` | `ph_calendar_check` |
| `calendar.badge.plus` | `calendar-plus` | `ph_calendar_plus` |
| `camera` | `camera` | `ph_camera` |
| `chart.bar` | `chart-bar` | `ph_chart_bar` |
| `chart.bar.xaxis` | `chart-bar-horizontal` | `ph_chart_bar_horizontal` |
| `checklist` | `list-checks` | `ph_list_checks` |
| `checkmark` | `check` | `ph_check` |
| `checkmark.circle` | `check-circle` | `ph_check_circle` |
| `checkmark.circle.fill` | `check-circle-fill` | `ph_check_circle_fill` |
| `checkmark.shield` | `shield-check` | `ph_shield_check` |
| `chevron.down` | `caret-down` | `ph_caret_down` |
| `chevron.left` | `caret-left` | `ph_caret_left` |
| `chevron.right` | `caret-right` | `ph_caret_right` |
| `chevron.up` | `caret-up` | `ph_caret_up` |
| `chevron.up.chevron.down` | `caret-up-down` | `ph_caret_up_down` |
| `circle` | `circle` | `ph_circle` |
| `circle.dotted.circle` | `circle-dashed` | `ph_circle_dashed` |
| `circle.fill` | `circle-fill` | `ph_circle_fill` |
| `clock` | `clock` | `ph_clock` |
| `clock.arrow.2.circlepath` | `clock-clockwise` | `ph_clock_clockwise` |
| `clock.arrow.circlepath` | `clock-counter-clockwise` | `ph_clock_counter_clockwise` |
| `cloud` | `cloud` | `ph_cloud` |
| `cpu` | `cpu` | `ph_cpu` |
| `creditcard` | `credit-card` | `ph_credit_card` |
| `crown` | `crown` | `ph_crown` |
| `crown.fill` | `crown-fill` | `ph_crown_fill` |
| `cursorarrow.motionlines` | `cursor-click` | `ph_cursor_click` |
| `desktopcomputer` | `desktop` | `ph_desktop` |
| `desktopcomputer.trianglebadge.exclamationmark` | `desktop` | `ph_desktop` |
| `display` | `monitor` | `ph_monitor` |
| `doc` | `file` | `ph_file` |
| `doc.badge.plus` | `file-plus` | `ph_file_plus` |
| `doc.on.doc` | `copy` | `ph_copy` |
| `doc.richtext` | `file-text` | `ph_file_text` |
| `doc.text` | `file-text` | `ph_file_text` |
| `doc.viewfinder` | `scan` | `ph_scan` |
| `dollarsign` | `currency-dollar` | `ph_currency_dollar` |
| `dot.radiowaves.left.and.right` | `broadcast` | `ph_broadcast` |
| `ellipsis` | `dots-three` | `ph_dots_three` |
| `envelope` | `envelope` | `ph_envelope` |
| `exclamationmark.circle` | `warning-circle` | `ph_warning_circle` |
| `exclamationmark.circle.fill` | `warning-circle-fill` | `ph_warning_circle_fill` |
| `exclamationmark.triangle` | `warning` | `ph_warning` |
| `exclamationmark.triangle.fill` | `warning-fill` | `ph_warning_fill` |
| `externaldrive` | `hard-drive` | `ph_hard_drive` |
| `externaldrive.badge.exclamationmark` | `hard-drive` | `ph_hard_drive` |
| `eye` | `eye` | `ph_eye` |
| `figure.walk.motion` | `person-simple-walk` | `ph_person_simple_walk` |
| `flag` | `flag` | `ph_flag` |
| `flame` | `flame` | `ph_flame` |
| `floppy.disk` | `floppy-disk` | `ph_floppy_disk` |
| `folder` | `folder` | `ph_folder` |
| `folder.badge.plus` | `folder-plus` | `ph_folder_plus` |
| `gamecontroller` | `game-controller` | `ph_game_controller` |
| `gamecontroller.fill` | `game-controller-fill` | `ph_game_controller_fill` |
| `gearshape` | `gear` | `ph_gear` |
| `globe` | `globe` | `ph_globe` |
| `graduationcap` | `graduation-cap` | `ph_graduation_cap` |
| `hand.raised` | `hand` | `ph_hand` |
| `hand.raised.fill` | `hand-fill` | `ph_hand_fill` |
| `hand.tap` | `hand-tap` | `ph_hand_tap` |
| `hard.drive` | `hard-drive` | `ph_hard_drive` |
| `hourglass` | `hourglass` | `ph_hourglass` |
| `house` | `house` | `ph_house` |
| `icloud.and.arrow.down` | `cloud-arrow-down` | `ph_cloud_arrow_down` |
| `info.circle` | `info` | `ph_info` |
| `iphone` | `device-mobile` | `ph_device_mobile` |
| `iphone.gen3` | `device-mobile` | `ph_device_mobile` |
| `iphone.radiowaves.left.and.right` | `device-mobile-speaker` | `ph_device_mobile_speaker` |
| `keyboard` | `keyboard` | `ph_keyboard` |
| `laptopcomputer` | `laptop` | `ph_laptop` |
| `line.3.horizontal` | `list` | `ph_list` |
| `line.3.horizontal.decrease` | `funnel-simple` | `ph_funnel_simple` |
| `line.3.horizontal.decrease.circle` | `funnel` | `ph_funnel` |
| `link` | `link` | `ph_link` |
| `link.badge.plus` | `link-simple` | `ph_link_simple` |
| `list.bullet` | `list-bullets` | `ph_list_bullets` |
| `list.bullet.rectangle` | `list-dashes` | `ph_list_dashes` |
| `lock` | `lock` | `ph_lock` |
| `lock.fill` | `lock-fill` | `ph_lock_fill` |
| `magnifyingglass` | `magnifying-glass` | `ph_magnifying_glass` |
| `map` | `map-trifold` | `ph_map_trifold` |
| `message` | `chat-teardrop` | `ph_chat_teardrop` |
| `mic.fill` | `microphone-fill` | `ph_microphone_fill` |
| `minus.circle.fill` | `minus-circle-fill` | `ph_minus_circle_fill` |
| `moon.fill` | `moon-fill` | `ph_moon_fill` |
| `network` | `network` | `ph_network` |
| `paintbrush` | `paint-brush` | `ph_paint_brush` |
| `paperclip` | `paperclip` | `ph_paperclip` |
| `paperplane` | `paper-plane-tilt` | `ph_paper_plane_tilt` |
| `pause.circle` | `pause-circle` | `ph_pause_circle` |
| `pause.fill` | `pause-fill` | `ph_pause_fill` |
| `pencil` | `pencil-simple` | `ph_pencil_simple` |
| `pencil.circle.fill` | `pencil-circle` | `ph_pencil_circle` |
| `person.badge.plus` | `user-plus` | `ph_user_plus` |
| `person.circle` | `user-circle` | `ph_user_circle` |
| `person.crop.circle` | `user-circle` | `ph_user_circle` |
| `person.crop.circle.badge.xmark` | `user-circle-minus` | `ph_user_circle_minus` |
| `phone` | `phone` | `ph_phone` |
| `photo` | `image` | `ph_image` |
| `photo.badge.exclamationmark` | `image-broken` | `ph_image_broken` |
| `photo.on.rectangle` | `images` | `ph_images` |
| `pin.slash` | `push-pin-slash` | `ph_push_pin_slash` |
| `play` | `play` | `ph_play` |
| `play.fill` | `play-fill` | `ph_play_fill` |
| `plus` | `plus` | `ph_plus` |
| `plus.circle` | `plus-circle` | `ph_plus_circle` |
| `plus.circle.fill` | `plus-circle-fill` | `ph_plus_circle_fill` |
| `plus.forwardslash.minus` | `plus-minus` | `ph_plus_minus` |
| `point.3.connected.trianglepath.dotted` | `graph` | `ph_graph` |
| `point.topleft.down.to.point.bottomright.curvepath` | `path` | `ph_path` |
| `powerplug` | `plug` | `ph_plug` |
| `puzzlepiece.extension` | `puzzle-piece` | `ph_puzzle_piece` |
| `puzzlepiece.extension.fill` | `puzzle-piece-fill` | `ph_puzzle_piece_fill` |
| `puzzlepiece.fill` | `puzzle-piece-fill` | `ph_puzzle_piece_fill` |
| `qrcode.viewfinder` | `qr-code` | `ph_qr_code` |
| `rectangle.portrait.and.arrow.right` | `sign-out` | `ph_sign_out` |
| `rectangle.split.3x1` | `columns` | `ph_columns` |
| `repeat` | `repeat` | `ph_repeat` |
| `return` | `arrow-elbow-down-left` | `ph_arrow_elbow_down_left` |
| `safari` | `compass` | `ph_compass` |
| `signature` | `signature` | `ph_signature` |
| `slider.horizontal.3` | `sliders-horizontal` | `ph_sliders_horizontal` |
| `sparkle` | `sparkle` | `ph_sparkle` |
| `sparkles` | `sparkle` | `ph_sparkle` |
| `sparkles.bubble.left.fill` | `chat-circle-dots-fill` | `ph_chat_circle_dots_fill` |
| `square` | `square` | `ph_square` |
| `square.and.arrow.down` | `download-simple` | `ph_download_simple` |
| `square.and.arrow.up` | `export` | `ph_export` |
| `square.and.pencil` | `note-pencil` | `ph_note_pencil` |
| `square.fill` | `square-fill` | `ph_square_fill` |
| `square.grid.2x2` | `squares-four` | `ph_squares_four` |
| `star` | `star` | `ph_star` |
| `star.fill` | `star-fill` | `ph_star_fill` |
| `stop.circle` | `stop-circle` | `ph_stop_circle` |
| `stop.fill` | `stop-fill` | `ph_stop_fill` |
| `storefront` | `storefront` | `ph_storefront` |
| `switch.2` | `toggle-left` | `ph_toggle_left` |
| `tablecells` | `table` | `ph_table` |
| `target` | `target` | `ph_target` |
| `terminal` | `terminal` | `ph_terminal` |
| `thermometer` | `thermometer` | `ph_thermometer` |
| `trash` | `trash` | `ph_trash` |
| `tray` | `tray` | `ph_tray` |
| `video` | `video-camera` | `ph_video_camera` |
| `video.badge.checkmark` | `video-camera` | `ph_video_camera` |
| `video.slash` | `video-camera-slash` | `ph_video_camera_slash` |
| `wallet` | `wallet` | `ph_wallet` |
| `waveform` | `waveform` | `ph_waveform` |
| `waveform.path.ecg` | `heartbeat` | `ph_heartbeat` |
| `wifi.exclamationmark` | `wifi-x` | `ph_wifi_x` |
| `wifi.slash` | `wifi-slash` | `ph_wifi_slash` |
| `wrench` | `wrench` | `ph_wrench` |
| `wrench.and.screwdriver` | `wrench` | `ph_wrench` |
| `xmark` | `x` | `ph_x` |
| `xmark.circle` | `x-circle` | `ph_x_circle` |
| `xmark.circle.fill` | `x-circle-fill` | `ph_x_circle_fill` |

## Retired hand-drawn glyphs

These per-module glyphs were drawn by hand in earlier waves. They are deleted and every
reference now points at the Phosphor drawable.

| Old drawable | Phosphor drawable |
|---|---|
| `shell_x` | `ph_x` |
| `shell_dots` | `ph_dots_three` |
| `shell_clock` | `ph_clock` |
| `shell_plus` | `ph_plus` |
| `shell_note_pencil` | `ph_note_pencil` |
| `shell_folder` | `ph_folder` |
| `shell_bolt` | `ph_lightning` |
| `shell_tray` | `ph_tray` |
| `shell_list` | `ph_list` |
| `shell_link` | `ph_link` |
| `shell_file_text` | `ph_file_text` |
| `shell_device_mobile` | `ph_device_mobile` |
| `shell_desktop` | `ph_desktop` |
| `shell_chat` | `ph_chat_circle` |
| `shell_wifi_slash` | `ph_wifi_slash` |
| `shell_star_fill` | `ph_star_fill` |
| `shell_star` | `ph_star` |
| `shell_stack` | `ph_stack` |
| `shell_shield_check` | `ph_shield_check` |
| `shell_photo` | `ph_image` |
| `shell_grid` | `ph_squares_four` |
| `shell_grad_cap` | `ph_graduation_cap` |
| `shell_caret_up` | `ph_caret_up` |
| `shell_caret_right` | `ph_caret_right` |
| `shell_waveform` | `ph_waveform` |
| `shell_scribble` | `ph_scribble_loop` |
| `shell_mic` | `ph_microphone` |
| `shell_house` | `ph_house` |
| `shell_file_plus` | `ph_file_plus` |
| `shell_checklist` | `ph_list_checks` |
| `shell_caret_down` | `ph_caret_down` |
| `shell_camera` | `ph_camera` |
| `shell_brain` | `ph_brain` |
| `shell_bell_slash` | `ph_bell_slash` |
| `bots_desktop` | `ph_desktop` |
| `bots_chat` | `ph_chat_circle` |
| `bots_caret_right` | `ph_caret_right` |
| `bots_x` | `ph_x` |
| `bots_tray` | `ph_tray` |
| `bots_pencil` | `ph_pencil_simple` |
| `bots_hand` | `ph_hand` |
| `bots_grid` | `ph_squares_four` |
| `bots_file_text` | `ph_file_text` |
| `bots_list` | `ph_list` |
| `bots_gear` | `ph_gear` |
| `bots_eye` | `ph_eye` |
| `bots_caret_down` | `ph_caret_down` |
| `bots_stop` | `ph_stop_fill` |
| `bots_star` | `ph_star` |
| `bots_question` | `ph_question` |
| `bots_plus` | `ph_plus` |
| `bots_link` | `ph_link` |
| `bots_house` | `ph_house` |
| `bots_dots` | `ph_dots_three` |
| `bots_clock` | `ph_clock` |
| `bots_star_fill` | `ph_star_fill` |
| `bots_photo` | `ph_image` |
| `bots_folder` | `ph_folder` |
| `bots_file_plus` | `ph_file_plus` |
| `bots_brain` | `ph_brain` |
| `chat_share` | `ph_export` |
| `chat_retry` | `ph_arrow_clockwise` |
| `chat_file` | `ph_file` |
| `chat_caret_up` | `ph_caret_up` |
| `chat_caret_down` | `ph_caret_down` |
| `chat_pencil` | `ph_pencil_simple` |
| `chat_gear` | `ph_gear` |
| `chat_thumbs_up` | `ph_thumbs_up` |
| `chat_thumbs_down` | `ph_thumbs_down` |
| `chat_stop` | `ph_stop_fill` |
| `chat_speaker` | `ph_speaker_high` |
| `chat_question` | `ph_question` |
| `chat_plugs` | `ph_plugs` |
| `chat_hand` | `ph_hand` |
| `chat_brain` | `ph_brain` |
| `vc_bell` | `ph_bell` |
| `vc_call_end` | `ph_phone_x` |
| `vc_camera` | `ph_camera` |
| `vc_chat` | `ph_chat_circle` |
| `vc_clock` | `ph_clock` |
| `vc_expand` | `ph_arrows_out` |
| `vc_gear` | `ph_gear` |
| `vc_mic_off` | `ph_microphone_slash` |
| `vc_mic` | `ph_microphone` |
| `vc_minimize` | `ph_arrows_in` |
| `vc_photo` | `ph_image` |
| `vc_speaker_off` | `ph_speaker_slash` |
| `vc_speaker` | `ph_speaker_high` |
| `vc_thumb_down` | `ph_thumbs_down` |
| `vc_thumb_up` | `ph_thumbs_up` |
| `vc_video` | `ph_video_camera` |
| `vc_waveform` | `ph_waveform` |
| `rfb_x` | `ph_x` |
| `rfb_keyboard` | `ph_keyboard` |
| `rfb_hand` | `ph_hand` |
| `rfb_eye` | `ph_eye` |
| `rfb_corners_out` | `ph_corners_out` |
