# Mobile parity

Desktop (`apps/desktop`, Rust GPUI) is the reference. iOS (`apps/mobile/apple`) and Android
(`apps/mobile/android`) share `crates/lagom-ffi`. Checked against the code on 2026-11-14, on
branch `mobile/parity-round`. Rows marked "round 2" were updated on branch `mobile/parity-round-2`; their line
numbers are for that branch. Rows marked "round 3" were updated on branch `mobile/parity-round-3`, with line
numbers for that branch. A "yes" means the code path
exists. It does not mean a hand test on a device.

Values: **yes**, **partial** (the note says what is missing) or **no**.

## Daily mail

| Feature                           | Desktop                                                                                       | iOS                                                                                                                                                           | Android                                                                                              |
| --------------------------------- | --------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| Read a message                    | yes, `main_window/conversation.rs`                                                            | yes, `Sources/MessageView.swift:24`                                                                                                                           | yes, `MailScreen.kt:424`                                                                             |
| Threads (conversation view)       | yes, `main_window/conversation.rs`                                                            | yes, `MailModel.swift` `select`, FFI `lib.rs:414`                                                                                                             | yes, `MailModel.kt` `select`, FFI `lib.rs:414`                                                       |
| Reply, Reply all, Forward         | yes, `command_palette.rs:89-92`                                                               | yes, `MessageView.swift:150-152`                                                                                                                              | yes, `MailScreen.kt:500-504`                                                                         |
| Compose with Cc                   | yes, `compose.rs:387` (Add Cc)                                                                | yes, `ComposeView.swift:55`                                                                                                                                   | yes, `Forms.kt:106`                                                                                  |
| Bcc                               | yes, only from a mailto link or a saved draft, `compose.rs:157,695`                           | yes (this PR), same rule, `ComposeView.swift:58`                                                                                                              | yes (this PR), same rule, `Forms.kt:107`                                                             |
| Attachments: send                 | yes, `compose.rs:44`                                                                          | yes, files and photos, `ComposeView.swift:133`                                                                                                                | yes, files and photos, `Forms.kt:83`                                                                 |
| Attachments: open received        | yes, `sync.rs` `attachment_bytes`                                                             | yes, `MessageView.swift:75`                                                                                                                                   | yes, `MailScreen.kt:482`                                                                             |
| Search                            | yes, `main_window.rs:1665`                                                                    | yes, `LagomApp.swift:154`, FFI `lib.rs:442`                                                                                                                   | yes, `MailScreen.kt:205`, FFI `lib.rs:442`                                                           |
| Done, Later, snooze choices       | yes, `command_palette.rs:31-32`                                                               | yes, `LagomApp.swift:112`                                                                                                                                     | yes, `MailScreen.kt:511`                                                                             |
| Undo send                         | yes, `compose.rs:26` (5 s)                                                                    | yes, `LagomApp.swift:96`                                                                                                                                      | yes, `MailScreen.kt:194`                                                                             |
| Undo Done, Later, Delete          | yes, `main_window.rs:1094` (Cmd-Z)                                                            | yes (this PR), 5 s notice, `MailModel.swift:566`                                                                                                              | yes (this PR), 5 s notice, `MailModel.kt:443`                                                        |
| Folders and labels (browse)       | yes, `folders.rs`, sidebar                                                                    | yes (round 2): sidebar Folders, `RootView.swift:163`; phone mailbox menu, `LagomApp.swift`; FFI `folders`, `folder_mail` `lib.rs:452`                         | yes (round 2): Mailboxes menu, `MailScreen.kt:126`; FFI `lib.rs:452`                                 |
| Move to a folder                  | yes, `command_palette.rs:119-154`                                                             | yes (round 2), with Undo: list menu and reading toolbar, `LagomApp.swift:139,354`; FFI `move_to_folder` `lib.rs:518`                                          | yes (round 2), with Undo: long press and reading view, `MailScreen.kt:354,572`                       |
| Create a folder                   | yes, "New folder" in the sidebar and Move to, `main_window.rs:1719`, `command_palette.rs:162` | yes (round 3): sidebar and phone mailbox menu, "New folder…" in Move to, `RootView.swift:184`, `LagomApp.swift:145,207,474`; FFI `create_folder` `lib.rs:619` | yes (round 3): Mailboxes menu, "New folder…" in Move to, `MailScreen.kt:128,382,586`, `Forms.kt:316` |
| Delete (to Trash)                 | yes, `command_palette.rs:33`                                                                  | yes, `LagomApp.swift:317`                                                                                                                                     | yes, `MailScreen.kt:521`                                                                             |
| Empty Trash                       | yes, `main_window.rs:352`                                                                     | yes (round 2), "Delete all permanently?", `LagomApp.swift:145`; FFI `empty_trash` `lib.rs:522`                                                                | yes (round 2), same words, `MailScreen.kt:232`                                                       |
| Mark read or unread               | yes, `command_palette.rs:91`                                                                  | yes, list and reading view, `LagomApp.swift:310,408`                                                                                                          | yes (round 2): long press and TalkBack action on the list, `MailScreen.kt:354`; reading view         |
| Swipe actions                     | yes, `main_window.rs:599` (trackpad)                                                          | yes, Done and Later/Delete, `LagomApp.swift:314-317`                                                                                                          | yes, Done and Later/Delete, `MailScreen.kt:299`                                                      |
| New-mail notifications            | yes, `notifications.rs:109`                                                                   | yes, `BackgroundMail.swift:6-25` (not while locked, see TODO)                                                                                                 | yes, `BackgroundMail.kt:29-44`                                                                       |
| Multiple accounts                 | yes, `accounts.rs`, `settings.rs:135`                                                         | yes, `AddAccountView.swift`, `LagomApp.swift:169`                                                                                                             | yes, `Forms.kt` `AddAccountScreen`, `MailScreen.kt:116-126`                                          |
| Unified inbox                     | yes, `mail.rs:207`                                                                            | yes, "All accounts", `LagomApp.swift:170`                                                                                                                     | yes, "All accounts", `MailScreen.kt:116`                                                             |
| Signatures                        | yes, `settings.rs:235`                                                                        | yes (this PR): edit, `AddAccountView.swift:139`; new mail, `MailModel.swift:381`                                                                              | yes (this PR): edit, `Forms.kt:266`; new mail, `MailModel.kt:300`                                    |
| HTML mail                         | yes, `main_window/html.rs`                                                                    | yes, `MessageView.swift:167,247`, FFI `lib.rs:664`                                                                                                            | yes, `MailHtml.kt`, FFI `lib.rs:664`                                                                 |
| Remote images blocked until asked | yes, `main_window/conversation.rs:61`                                                         | yes, `MessageView.swift:70`                                                                                                                                   | yes, `MailScreen.kt:473`                                                                             |
| Unsubscribe                       | yes, `main_window.rs:2240`                                                                    | yes, `MessageView.swift:115`                                                                                                                                  | yes, `MailScreen.kt:373`                                                                             |
| Drafts                            | yes, `main_window.rs:322`                                                                     | yes, `MailModel.swift` `openDraft`                                                                                                                            | yes, `MailScreen.kt:112`                                                                             |
| mailto links and shares           | yes, `main_window.rs` compose `link`                                                          | yes, `RootView.swift:62`                                                                                                                                      | yes, `MainActivity.kt:128`                                                                           |
| Account name ("Your name")        | yes, `main_window/live.rs:1433`                                                               | yes (round 2), account menu, `LagomApp.swift:215`; FFI `set_account_name` `lib.rs:300`                                                                        | yes (round 2), `Forms.kt:294`                                                                        |

## Calendar and todo

| Feature    | Desktop                     | iOS                                                           | Android                                                                                                                                       |
| ---------- | --------------------------- | ------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| Today page | yes, `main_window/home.rs`  | yes, `PlannerViews.swift:10`                                  | yes (android/planner), `PlannerScreen.kt:118`; page bar `MainActivity.kt:214`                                                                 |
| Calendar   | yes, `main_window/week.rs`  | partial: needs a Google account, `PlannerViews.swift:368,404` | partial (round 3): the phone day list, `PlannerScreen.kt:390`; the iPad week grid on tablets, `PlannerScreen.kt:438`; same Google empty state |
| Todo lists | yes, `main_window/tasks.rs` | yes, `PlannerViews.swift:231,312`                             | yes (android/planner), `PlannerScreen.kt:236`                                                                                                 |

## Gaps, ranked by daily impact

1. **Undo for Done, Later and Delete.** A wrong swipe had no way back on mobile. Closed in this PR.
2. **Signatures.** Mobile had no editor, and new mail started empty. Closed in this PR.
3. **Folders and Move to folder.** Mobile showed only the fixed mailboxes. Closed in round 2: FFI folder
   list, folder mail and a move with Undo, a picker on both platforms.
4. **Android Today, Calendar and Todo.** Closed on branch `android/planner`: the page bar, Today, Todo and the phone
   Calendar list. The tablet week grid closed in round 3.
5. **Create a folder on mobile.** Closed in round 3: "New folder" with desktop's name checks, in the folder
   list and in Move to, on both platforms.
6. **Calendar without Google on iOS.** Waits for the OAuth clients (Alex, see TODO).
7. **Mark read or unread from the Android list.** Closed in round 2: a long press opens the same choices as the iOS context menu.
8. **Empty Trash on mobile.** Closed in round 2, with desktop's confirm words.
9. **Bcc from a link or a saved draft.** Mobile dropped it with a notice. Closed in this PR, with the desktop rule.
10. **Account name editing on mobile.** Closed in round 2.
11. **iOS background refresh while locked.** A file-protection and Keychain decision (Alex, see TODO).

## Known limits of this round

- Undo after Done or Delete finds the message again by its Message-ID, as desktop does. Mail
  without a Message-ID gets no Undo.
- Undo moves the message back with IMAP MOVE (or COPY and delete), the same call desktop uses.
  Gmail's All Mail behaviour on that move is not verified here.
- The mobile Undo notice lasts 5 s. Desktop keeps an undo history for Cmd-Z.

## Known limits of round 2

- Mobile cannot create a folder. Desktop offers "New folder" in Move to. Closed in round 3.
- Folders show as a flat list with their path ("Travel / Trains"). The iPad sidebar indents subfolders.
  Folders do not collapse.
- Gmail labels show as folders, as on desktop. A move on Gmail is a label change on the server. Not verified
  against Gmail here.
- Empty Trash needs the server's UIDPLUS, as on desktop. Without it the app shows the core's error.
- In "All accounts", Empty Trash empties every account's Trash. One account's failure does not stop the others.
- Nothing here ran on a device or against a real server. The Rust tests use a loopback IMAP fixture.

## Known limits of the Android planner (branch `android/planner`)

- Calendar shows the phone day list on every screen size. The iPad week grid has no Android version yet. Closed in round 3.
- The mobile core never fetches a calendar, on iOS or Android. Calendar stays empty outside demo data.
  Android says "Google Calendar sync is coming to Android" where iOS says "Google sign-in is coming to iPhone and iPad".
- Android shows no account colour on events, because the Android mail list has no account colours.
- Todo deletes a task with a swipe to the start or the TalkBack "Delete" action, as iOS. There is no Undo, as on iOS.
- A rotation while "Add task" is saving keeps the dialog open with its text. A second Add then makes a second task.
- Nothing here ran on a device or an emulator. JVM tests cover `PlannerModel`, `DateWords` and `openInbox`.

## Known limits of round 3

- "New folder" makes a top-level folder, as on desktop. A name with the server's hierarchy delimiter is refused
  ("Folder names cannot contain the hierarchy delimiter"). The checks live in core (`validate_folder_name`), so
  desktop, iOS and Android refuse the same names with the same words.
- A name that the server treats as a special folder (for example "Archive" on a server without one) is created,
  but the folder list hides it, as desktop's does.
- The Android week grid uses the tablet breakpoint of the mail two-pane layout: width not compact and height not
  compact. A phone in landscape keeps the day list, as an iPhone does.
- The Android week grid has no account colours, as the Android day list. Event details open in a dialog where iPad
  uses a popover.
- Nothing here ran on a device, an emulator or against a real server. The Rust test uses a loopback IMAP fixture.
  JVM tests cover the folder model and the week-grid lane layout; the Swift harness covers the iOS folder model.
