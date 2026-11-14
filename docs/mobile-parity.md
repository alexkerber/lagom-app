# Mobile parity

Desktop (`apps/desktop`, Rust GPUI) is the reference. iOS (`apps/mobile/apple`) and Android
(`apps/mobile/android`) share `crates/lagom-ffi`. Checked against the code on 2026-11-14, on
branch `mobile/parity-round`. Line numbers are for that branch. A "yes" means the code path
exists. It does not mean a hand test on a device.

Values: **yes**, **partial** (the note says what is missing) or **no**.

## Daily mail

| Feature                           | Desktop                                                             | iOS                                                                                             | Android                                                           |
| --------------------------------- | ------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| Read a message                    | yes, `main_window/conversation.rs`                                  | yes, `Sources/MessageView.swift:24`                                                             | yes, `MailScreen.kt:424`                                          |
| Threads (conversation view)       | yes, `main_window/conversation.rs`                                  | yes, `MailModel.swift` `select`, FFI `lib.rs:414`                                               | yes, `MailModel.kt` `select`, FFI `lib.rs:414`                    |
| Reply, Reply all, Forward         | yes, `command_palette.rs:89-92`                                     | yes, `MessageView.swift:150-152`                                                                | yes, `MailScreen.kt:500-504`                                      |
| Compose with Cc                   | yes, `compose.rs:387` (Add Cc)                                      | yes, `ComposeView.swift:55`                                                                     | yes, `Forms.kt:106`                                               |
| Bcc                               | yes, only from a mailto link or a saved draft, `compose.rs:157,695` | yes (this PR), same rule, `ComposeView.swift:58`                                                | yes (this PR), same rule, `Forms.kt:107`                          |
| Attachments: send                 | yes, `compose.rs:44`                                                | yes, files and photos, `ComposeView.swift:133`                                                  | yes, files and photos, `Forms.kt:83`                              |
| Attachments: open received        | yes, `sync.rs` `attachment_bytes`                                   | yes, `MessageView.swift:75`                                                                     | yes, `MailScreen.kt:482`                                          |
| Search                            | yes, `main_window.rs:1665`                                          | yes, `LagomApp.swift:154`, FFI `lib.rs:442`                                                     | yes, `MailScreen.kt:205`, FFI `lib.rs:442`                        |
| Done, Later, snooze choices       | yes, `command_palette.rs:31-32`                                     | yes, `LagomApp.swift:112`                                                                       | yes, `MailScreen.kt:511`                                          |
| Undo send                         | yes, `compose.rs:26` (5 s)                                          | yes, `LagomApp.swift:96`                                                                        | yes, `MailScreen.kt:194`                                          |
| Undo Done, Later, Delete          | yes, `main_window.rs:1094` (Cmd-Z)                                  | yes (this PR), 5 s notice, `MailModel.swift:566`                                                | yes (this PR), 5 s notice, `MailModel.kt:443`                     |
| Folders and labels (browse)       | yes, `folders.rs`, sidebar                                          | partial: Inbox, Later, Done, Sent, Trash and Drafts only, `LagomApp.swift:166`, FFI `lib.rs:68` | partial: same five plus Drafts, `MailScreen.kt:107-112`           |
| Move to a folder                  | yes, `command_palette.rs:119-154`                                   | no                                                                                              | no                                                                |
| Delete (to Trash)                 | yes, `command_palette.rs:33`                                        | yes, `LagomApp.swift:317`                                                                       | yes, `MailScreen.kt:521`                                          |
| Empty Trash                       | yes, `main_window.rs:352`                                           | no                                                                                              | no                                                                |
| Mark read or unread               | yes, `command_palette.rs:91`                                        | yes, list and reading view, `LagomApp.swift:310,408`                                            | partial: reading view only, `MailScreen.kt:505`                   |
| Swipe actions                     | yes, `main_window.rs:599` (trackpad)                                | yes, Done and Later/Delete, `LagomApp.swift:314-317`                                            | yes, Done and Later/Delete, `MailScreen.kt:299`                   |
| New-mail notifications            | yes, `notifications.rs:109`                                         | yes, `BackgroundMail.swift:6-25` (not while locked, see TODO)                                   | yes, `BackgroundMail.kt:29-44`                                    |
| Multiple accounts                 | yes, `accounts.rs`, `settings.rs:135`                               | yes, `AddAccountView.swift`, `LagomApp.swift:169`                                               | yes, `Forms.kt` `AddAccountScreen`, `MailScreen.kt:116-126`       |
| Unified inbox                     | yes, `mail.rs:207`                                                  | yes, "All accounts", `LagomApp.swift:170`                                                       | yes, "All accounts", `MailScreen.kt:116`                          |
| Signatures                        | yes, `settings.rs:235`                                              | yes (this PR): edit, `AddAccountView.swift:139`; new mail, `MailModel.swift:381`                | yes (this PR): edit, `Forms.kt:266`; new mail, `MailModel.kt:300` |
| HTML mail                         | yes, `main_window/html.rs`                                          | yes, `MessageView.swift:167,247`, FFI `lib.rs:664`                                              | yes, `MailHtml.kt`, FFI `lib.rs:664`                              |
| Remote images blocked until asked | yes, `main_window/conversation.rs:61`                               | yes, `MessageView.swift:70`                                                                     | yes, `MailScreen.kt:473`                                          |
| Unsubscribe                       | yes, `main_window.rs:2240`                                          | yes, `MessageView.swift:115`                                                                    | yes, `MailScreen.kt:373`                                          |
| Drafts                            | yes, `main_window.rs:322`                                           | yes, `MailModel.swift` `openDraft`                                                              | yes, `MailScreen.kt:112`                                          |
| mailto links and shares           | yes, `main_window.rs` compose `link`                                | yes, `RootView.swift:62`                                                                        | yes, `MainActivity.kt:128`                                        |
| Account name ("Your name")        | yes, `main_window/live.rs:1433`                                     | no                                                                                              | no                                                                |

## Calendar and todo

| Feature    | Desktop                     | iOS                                                           | Android |
| ---------- | --------------------------- | ------------------------------------------------------------- | ------- |
| Today page | yes, `main_window/home.rs`  | yes, `PlannerViews.swift:10`                                  | no      |
| Calendar   | yes, `main_window/week.rs`  | partial: needs a Google account, `PlannerViews.swift:368,404` | no      |
| Todo lists | yes, `main_window/tasks.rs` | yes, `PlannerViews.swift:231,312`                             | no      |

## Gaps, ranked by daily impact

1. **Undo for Done, Later and Delete.** A wrong swipe had no way back on mobile. Closed in this PR.
2. **Signatures.** Mobile had no editor, and new mail started empty. Closed in this PR.
3. **Folders and Move to folder.** Mobile shows only the fixed mailboxes. Needs an FFI folder
   list, a move call and a picker on both platforms. Next round.
4. **Android Today, Calendar and Todo.** iOS has them; Android has none. Large.
5. **Calendar without Google on iOS.** Waits for the OAuth clients (Alex, see TODO).
6. **Mark read or unread from the Android list.** iOS has a context menu; Android has the reading view only.
7. **Empty Trash on mobile.**
8. **Bcc from a link or a saved draft.** Mobile dropped it with a notice. Closed in this PR, with the desktop rule.
9. **Account name editing on mobile.**
10. **iOS background refresh while locked.** A file-protection and Keychain decision (Alex, see TODO).

## Known limits of this round

- Undo after Done or Delete finds the message again by its Message-ID, as desktop does. Mail
  without a Message-ID gets no Undo.
- Undo moves the message back with IMAP MOVE (or COPY and delete), the same call desktop uses.
  Gmail's All Mail behaviour on that move is not verified here.
- The mobile Undo notice lasts 5 s. Desktop keeps an undo history for Cmd-Z.
