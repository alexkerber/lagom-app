# App Store listing (draft)

The text for the App Store Connect listing of `com.alexkerber.lagom`. `en-US/` uses the file names
of fastlane `deliver`, so a later upload script can read them as they are. Paste them by hand until then.

| File                         | Limit | Now  |
| ---------------------------- | ----- | ---- |
| `en-US/name.txt`             | 30    | 5    |
| `en-US/subtitle.txt`         | 30    | 26   |
| `en-US/promotional_text.txt` | 170   | 129  |
| `en-US/description.txt`      | 4000  | 1143 |
| `en-US/keywords.txt`         | 100   | 97   |
| `en-US/support_url.txt`      | URL   |      |
| `en-US/privacy_url.txt`      | URL   |      |
| `en-US/marketing_url.txt`    | URL   |      |

Before you submit:

- The name "lagom" can be taken on the App Store. Then use "lagom mail".
- `lagommail.app/support` and `lagommail.app/privacy` do not exist yet. Make both pages on the site, or
  point the files at pages that exist. App Review opens both.
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

App Store Connect needs one iPhone set and one iPad set. It scales them down for smaller devices.

| Set         | Device                  | Portrait size (px) |
| ----------- | ----------------------- | ------------------ |
| iPhone 6.9" | iPhone 16 Pro Max class | 1320 × 2868        |
| iPad 13"    | iPad Pro 13" (M4) class | 2064 × 2752        |

Use the demo store (`-LagomDemo` launch argument, debug builds) so no real mail shows. Take each shot in
light mode. Add one dark shot at the end of each set. No captions on the image, or one short line at most.

1. **Inbox.** Unified Inbox with a few unread rows and the "N left" count. Caption: "Get to an empty inbox."
2. **Reading.** One message open with Done, Reply and Later in reach. Caption: "Done, Reply, Later."
3. **Later.** The Later list with a few messages. Caption: "Later, when you have time."
4. **Writing.** A reply in Markdown, with Cc. Caption: "Write in plain text or Markdown."
5. **Today.** Unread count, tasks and what is next. Caption: "Your day, on one page."
6. **Dark.** The inbox in dark mode. Caption: "Light and dark."

On iPad, show the sidebar and the list and the message side by side in shots 1, 2 and 6.

## Review notes (per release)

- A test IMAP account with its password, and the server names if discovery does not find them.
- "lagom has no account of its own. Tap Add account on the first screen and enter the test account."
