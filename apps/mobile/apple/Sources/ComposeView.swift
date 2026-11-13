import PhotosUI
import SwiftUI

/// The desktop compose sheet: From, To, Cc and Subject lines, the body, and Send with a word count.
struct ComposeView: View {
    let model: MailModel
    @State var draft: ComposeDraft
    @State private var sending = false
    @State private var error: String?
    @State private var cancelling = false
    @State private var contactsAccount: String?
    @State private var contactsError: String?
    @FocusState private var addressFocus: String?
    // Suggestions follow the caret: editing an earlier recipient must not complete the last one.
    @State private var addressSelection: [String: TextSelection] = [:]
    @State private var pickingFiles = false
    @State private var pickingPhotos = false
    @State private var photos: [PhotosPickerItem] = []
    @State private var attaching = false
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        SheetScaffold(title: draft.title, cancelDisabled: sending, cancel: { cancelling = true }) {
            VStack(spacing: 0) {
                FieldRow(label: "From") {
                    Menu {
                        Picker("From", selection: Binding(get: { draft.account }, set: { account in
                            sending = true
                            Task {
                                defer { sending = false }
                                do {
                                    model.composeDraft = draft
                                    try await model.changeComposeAccount(account)
                                    if let updated = model.composeDraft { draft = updated }
                                } catch { self.error = error.localizedDescription }
                            }
                        })) {
                            ForEach(model.accounts, id: \.id) { Text($0.email).tag($0.id) }
                        }
                    } label: {
                        HStack(spacing: 4) {
                            Text(from).lineLimit(1)
                            if !draft.isReply && !draft.restored { Icon("chevron", size: 14).foregroundStyle(Theme.secondary) }
                        }
                        .foregroundStyle(Theme.ink)
                        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading).contentShape(Rectangle())
                    }
                    .disabled(draft.isReply || draft.restored)
                    .accessibilityLabel("From")
                    .accessibilityValue(from)
                }
                FieldRow(label: "To") { address("To", binding(\.to)) }
                suggestions("To", binding(\.to))
                FieldRow(label: "Cc") { address("Cc", binding(\.cc)) }
                suggestions("Cc", binding(\.cc))
                if let contactsError { Text(contactsError).lagomFont(.small).foregroundStyle(Theme.danger) }
                FieldRow(label: "Subject") {
                    TextField("Subject", text: binding(\.subject), prompt: Text("")).fontWeight(.semibold).accessibilityLabel("Subject")
                }
                if !draft.attachments.isEmpty || attaching {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 6) {
                            ForEach(Array(draft.attachments.enumerated()), id: \.offset) { index, file in
                                HStack(spacing: 2) {
                                    Icon("attachment", size: 16).foregroundStyle(Theme.secondary)
                                    Text(file.filename).lagomFont(.small).lineLimit(1).truncationMode(.middle).frame(maxWidth: 180)
                                    IconButton(title: "Remove \(file.filename)", icon: "close") {
                                        draft.attachments.remove(at: index)
                                        model.composeDraft = draft
                                    }
                                }
                                .padding(.leading, 10)
                                .background(Theme.subtle, in: Capsule())
                            }
                            if attaching { ProgressView().accessibilityLabel("Adding attachments") }
                        }
                        .padding(.vertical, 6)
                    }
                    if draft.attachmentsTooLarge {
                        Text("Over 20 MB. Some mail servers refuse mail this large.").lagomFont(.small).foregroundStyle(Theme.secondary)
                            .lineLimit(1).minimumScaleFactor(0.8).frame(maxWidth: .infinity, alignment: .leading).padding(.bottom, 6)
                    }
                }
            }
            .padding(.horizontal, 16)
            TextEditor(text: binding(\.body))
                .lagomFont(.body).lineSpacing(3)
                .scrollContentBackground(.hidden)
                .overlay(alignment: .topLeading) {
                    if draft.body.isEmpty {
                        Text("Message · Markdown supported").lagomFont(.body).foregroundStyle(Theme.secondary)
                            .padding(.top, 8).padding(.leading, 5).allowsHitTesting(false).accessibilityHidden(true)
                    }
                }
                .padding(.horizontal, 12).padding(.vertical, 8)
                .accessibilityLabel("Message body")
        } footer: {
            HouseButton(title: "Send", icon: "send", primary: true) {
                Task {
                    guard model.core != nil else { return }
                    sending = true
                    defer { sending = false }
                    do {
                        model.composeDraft = draft
                        try await model.sendCompose()
                        dismiss()
                    } catch { self.error = error.localizedDescription }
                }
            }
            .disabled(sending || attaching || (draft.to.isEmpty && draft.cc.isEmpty) || draft.account.isEmpty)
            Menu {
                Button("Files") { pickingFiles = true }
                Button("Photos") { pickingPhotos = true }
            } label: {
                Icon("attachment").foregroundStyle(Theme.secondary).frame(minWidth: 44, minHeight: 44).contentShape(Rectangle())
            }
            .disabled(attaching)
            .accessibilityLabel("Attach")
            if sending { ProgressView().accessibilityLabel("Sending") }
            Spacer(minLength: 0)
            if let error {
                Text(error).lagomFont(.snippet).foregroundStyle(Theme.danger).lineLimit(2)
            } else {
                let words = draft.body.split(whereSeparator: \.isWhitespace).count
                Text(words == 1 ? "1 word" : "\(words) words").lagomFont(.small).monospacedDigit().foregroundStyle(Theme.secondary)
            }
        }
        .fileImporter(isPresented: $pickingFiles, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            switch result {
            case .success(let urls): attach { try await Task.detached { try urls.map(readAttachment) }.value }
            case .failure(let failure): error = failure.localizedDescription
            }
        }
        .photosPicker(isPresented: $pickingPhotos, selection: $photos, matching: .any(of: [.images, .videos]), preferredItemEncoding: .compatible)
        .onChange(of: photos) {
            let items = photos
            guard !items.isEmpty else { return }
            photos = []
            attach {
                var files: [SendAttachment] = []
                for item in items {
                    guard let file = try await item.loadTransferable(type: PickedFile.self) else { throw CocoaError(.fileReadUnknown) }
                    files.append(file.attachment)
                }
                return files
            }
        }
        .onDisappear { model.composeDisappeared(draft) }
        .confirmationDialog("Save draft?", isPresented: $cancelling, titleVisibility: .visible) {
            Button("Keep draft") { close(discard: false) }
            Button("Discard", role: .destructive) { close(discard: true) }
            Button("Cancel", role: .cancel) { }
        }
        .disabled(sending)
        .interactiveDismissDisabled(sending)
        .task(id: draft.account) {
            contactsAccount = nil
            contactsError = nil
            guard let core = model.core else { return }
            let account = draft.account
            do {
                try await core.refreshContacts(accountId: account)
                guard !Task.isCancelled else { return }
                contactsAccount = account
            } catch {
                if !Task.isCancelled { contactsError = "Could not load contacts. Enter an address to continue." }
            }
        }
    }

    private func close(discard: Bool) {
        sending = true
        Task {
            defer { sending = false }
            do {
                model.composeDraft = draft
                try await model.closeCompose(discard: discard)
                dismiss()
            } catch { self.error = error.localizedDescription }
        }
    }

    private func attach(_ read: @escaping () async throws -> [SendAttachment]) {
        attaching = true
        Task {
            defer { attaching = false }
            do {
                let files = try await read()
                // The sheet may have closed while reading; adding then would reopen it.
                guard model.composeDraft?.id == draft.id else { return }
                draft.attachments += files
                model.composeDraft = draft
            } catch { self.error = "Could not add the attachment: \(error.localizedDescription)" }
        }
    }

    private var from: String { model.accounts.first { $0.id == draft.account }?.email ?? "" }

    private func binding(_ field: WritableKeyPath<ComposeDraft, String>) -> Binding<String> {
        Binding(get: { draft[keyPath: field] }, set: { value in
            draft[keyPath: field] = value
            model.composeDraft = draft
        })
    }

    private func address(_ label: String, _ text: Binding<String>) -> some View {
        TextField(label, text: text, selection: Binding(get: { addressSelection[label] }, set: { addressSelection[label] = $0 }), prompt: Text(""))
            .textContentType(.emailAddress).keyboardType(.emailAddress)
            .textInputAutocapitalization(.never).autocorrectionDisabled()
            .focused($addressFocus, equals: label)
            .accessibilityLabel(label)
    }

    @ViewBuilder private func suggestions(_ label: String, _ text: Binding<String>) -> some View {
        let atEnd: Bool = {
            guard let selection = addressSelection[label] else { return true }
            if case .selection(let range) = selection.indices { return range.isEmpty && range.lowerBound == text.wrappedValue.endIndex }
            return false
        }()
        let query = atEnd ? recipientToken(text.wrappedValue) : ""
        if addressFocus == label, !query.isEmpty, contactsAccount == draft.account, let core = model.core {
            let matches = core.suggestContacts(accountId: draft.account, query: query)
            if !matches.isEmpty {
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(matches, id: \.address) { contact in
                            Button {
                                let next = recipientToken(text.wrappedValue, replacingWith: contact.recipient)
                                text.wrappedValue = next
                                addressSelection[label] = TextSelection(insertionPoint: next.endIndex)
                                addressFocus = label
                            } label: {
                                VStack(alignment: .leading, spacing: 2) {
                                    if !contact.name.isEmpty { Text(contact.name).foregroundStyle(Theme.ink) }
                                    Text(contact.address).lagomFont(.small).foregroundStyle(Theme.secondary)
                                }
                                .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading).contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("\(contact.name), \(contact.address)")
                            .accessibilityHint("Add recipient")
                        }
                    }
                }
                .frame(minHeight: 44, maxHeight: CGFloat(matches.count) * 44)
            }
        }
    }
}

/// A picked photo or video, read from the file the picker hands over so it keeps its name and type.
private struct PickedFile: Transferable {
    let attachment: SendAttachment
    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(importedContentType: .image) { PickedFile(attachment: try readAttachment($0.file)) }
        FileRepresentation(importedContentType: .movie) { PickedFile(attachment: try readAttachment($0.file)) }
    }
}
