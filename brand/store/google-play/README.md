# Google Play listing (draft)

Draft answers for the Play Console. A human pastes them in and confirms each one. The text files in
`en-US/` follow the fastlane `supply` layout, so a later upload script can read them as they are.

| Field             | File                           | Limit |
| ----------------- | ------------------------------ | ----- |
| App name          | `en-US/title.txt`              | 30    |
| Short description | `en-US/short_description.txt`  | 80    |
| Full description  | `en-US/full_description.txt`   | 4000  |
| App icon          | `../play-icon-512.png`         |       |
| Feature graphic   | `../play-feature-1024x500.png` |       |

Category: Communication. Contact email and privacy policy URL: a human fills them in.

## Data safety

Derived from the code on `main` (November 2026). Check again when the app adds a server, analytics,
crash reporting or OAuth.

What the app sends off the device:

- Mail, contacts and the account password go to the IMAP and SMTP servers of the account the user
  adds. lagom runs no server of its own. The developer never receives this data.
- During account setup, `autoconfig.thunderbird.net` receives the email domain (`example.com`), never
  the address or the password (`crates/lagom-core/src/discovery.rs`). The MX lookup uses the system DNS.
- Remote images in a message load only after the user taps "Load remote images"
  (`MailHtml.kt` blocks network loads until then).
- No analytics, no crash reporting, no ads and no advertising ID. The app has no SDK that sends data.

| Question                                                   | Answer | Why                                                                                                                                                                                                                                                  |
| ---------------------------------------------------------- | ------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Does the app collect or share any required user data type? | No     | Data goes only from the device to the user's own mail provider, at the user's request. The developer receives nothing. The domain lookup is processed ephemerally by a public service. Human decision: confirm this reading of Google's definitions. |
| Is all user data encrypted in transit?                     | Yes    | TLS or STARTTLS for IMAP and SMTP. Plain connections work only to the same device (`imap_sync.rs`). `usesCleartextTraffic="false"`. HTTPS for autoconfig.                                                                                            |
| Can users request that their data is deleted?              | Yes    | Removing an account deletes its password, its mail and its contacts from the device. Uninstalling deletes everything. Mail on the server stays under the provider's control.                                                                         |
| Data on the device                                         | -      | The password is encrypted with an Android Keystore key. Backups and device transfer exclude all app data (`data_extraction_rules.xml`, `allowBackup="false"`).                                                                                       |

If a reviewer reads "collection" to include the user's own mail server, the fallback answer is:
Personal info (email address) and Messages (emails), collected, not shared, required for app
functionality, encrypted in transit, deletable.

## Content rating (IARC questionnaire)

- Category: "All other app types" (communication).
- Violence, sexuality, language, controlled substances, gambling: No. The app shows only the user's own mail.
- Users can interact or exchange content with other users: Yes, through email. The app does not
  moderate it; it is the user's own mailbox.
- Shares the user's location: No. Digital purchases: No. Ads: No.
- Expected rating: Everyone / PEGI 3 with the "Users Interact" descriptor.

Target audience: 18 and over. The app is not designed for children.

## Screenshot shot list

Use a demo IMAP account with made-up senders. Never show real mail. Light theme first, then one dark
shot per device. Status bar clean: full battery, no notifications, time 09:41.

| #   | Screen                                       | Phone (1080×1920 or taller, 9:16) | 7" tablet (portrait) | 10" tablet (landscape, two panes) |
| --- | -------------------------------------------- | --------------------------------- | -------------------- | --------------------------------- |
| 1   | Inbox with eight to ten messages, two unread | yes                               | yes                  | list and open message             |
| 2   | Open message: plain text, one attachment     | yes                               | yes                  | -                                 |
| 3   | Later choices sheet over a message           | yes                               | yes                  | yes                               |
| 4   | Reply draft with the keyboard closed         | yes                               | -                    | yes                               |
| 5   | Undo send bar after sending                  | yes                               | -                    | -                                 |
| 6   | Empty inbox                                  | yes                               | yes                  | yes                               |
| 7   | Dark theme: inbox                            | yes                               | yes                  | yes                               |

Play needs 2 to 8 phone screenshots, and tablet screenshots for the tablet listing. Sides between
320 and 3840 px, no side more than twice the other. PNG or JPEG without transparency.
