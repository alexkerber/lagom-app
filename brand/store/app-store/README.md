# App Store listing (draft)

The text for the App Store Connect listing of `com.alexkerber.lagom`. `en-US/` uses the file names
of fastlane `deliver`, so a later upload script can read them as they are. Paste them by hand until then.

| File                         | Limit | Now  |
| ---------------------------- | ----- | ---- |
| `en-US/name.txt`             | 30    | 10   |
| `en-US/subtitle.txt`         | 30    | 26   |
| `en-US/promotional_text.txt` | 170   | 157  |
| `en-US/description.txt`      | 4000  | 1680 |
| `en-US/keywords.txt`         | 100   | 96   |
| `en-US/support_url.txt`      | URL   |      |
| `en-US/privacy_url.txt`      | URL   |      |
| `en-US/marketing_url.txt`    | URL   |      |

Before you submit:

- The name "lagom" can be taken on the App Store. Then use "lagom mail".
- `lagommail.app/support` and `lagommail.app/privacy` are live (checked 2026-11-17). App Review opens both.
- The description names only what the iOS app does on `main`. Google and Microsoft sign-in are not in
  the iOS app yet, so the copy says "Gmail works with an app-specific password". Update it when they land.

## Age rating answers

Every content question is **None** or **No**: no violence, sexual content, profanity, horror, drugs,
alcohol, tobacco, gambling, contests, medical content or weapons.

| Question                | Answer | Why                                                             |
| ----------------------- | ------ | --------------------------------------------------------------- |
| Unrestricted web access | No     | Links in mail open in Safari, not in the app.                   |
| User-generated content  | No     | No public feed or shared content. Mail goes between two people. |
| Messaging and chat      | Yes    | It is an email app.                                             |
| Advertising             | No     |                                                                 |
| Parental controls       | No     |                                                                 |
| Age assurance           | No     |                                                                 |

Apple computes the rating from these answers. Expect 4+, or a higher rating because of messaging.
Not verified against the questionnaire in App Store Connect today.

## App Privacy answers

Answer: **Data Not Collected.**

Apple counts data as collected when it leaves the device to the developer or a partner. What the code does:

- Mail, contacts from mail, drafts and the account password go only between the device and the mail
  server the user types in. The password stays in the iOS Keychain.
- No analytics, no crash reporter, no ads, no third-party SDK. lagom runs no server.
- Email-first setup sends the **domain** of the address (not the address) to
  `autoconfig.thunderbird.net` (Mozilla) to look up the server settings. lagom keeps nothing from it.
  Decision for the owner: treat this as not collected (no identifier, one lookup) or mention it in the
  privacy policy. The privacy policy should mention it either way.
- `PrivacyInfo.xcprivacy` declares no collected data and no tracking, to match.

## Screenshots: shot list

Glow style (Alex 2026-11-18): the app icon's gradient, a white Hauora caption and a 3D device from burk
(`~/Developer/burken/film/mockup.py`). `compose/compose.py` renders each slide at the store size.

| Set         | Size (px)   | Device in the slide           |
| ----------- | ----------- | ----------------------------- |
| iPhone 6.9" | 1320 × 2868 | iPhone Pro model              |
| iPad 13"    | 2064 × 2752 | iPad Pro model                |
| Mac         | 2880 × 1800 | MacBook model, window on glow |

Capture from the demo store (`-LagomDemo`, debug builds; `-LagomDemoOpen` opens the first message) at a
daytime hour, with the status bar at the real time so the now line and the clock agree.

1. **Mail.** "Done, Reply or Later." / "Every message gets one answer."
2. **Today.** "Your day in one sentence." / "Unread mail, events and tasks at a glance."
3. **Calendar.** "Your week at a glance." / "Your calendar, next to your mail."
4. **To-do.** "Lists that stay simple." / "Turn a message into a task."
5. **Dark mail.** "Calm, day and night." / "Private by default. No tracking, no ads."

## Review notes (per release)

- A test IMAP account with its password, and the server names if discovery does not find them.
- "lagom has no account of its own. Tap Add account on the first screen and enter the test account."
