import BackgroundTasks
import UIKit
import UserNotifications

@MainActor
final class MailAppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    let model = MailModel()
    private var requestingPermission = false
    static let refreshIdentifier = "app.lagommail.refresh"

    func application(_ application: UIApplication, didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        BGTaskScheduler.shared.register(forTaskWithIdentifier: Self.refreshIdentifier, using: .main) { task in
            self.refresh(task)
        }
        return true
    }

    static func schedule() {
        let request = BGAppRefreshTaskRequest(identifier: refreshIdentifier)
        request.earliestBeginDate = Date(timeIntervalSinceNow: 15 * 60)
        try? BGTaskScheduler.shared.submit(request)
    }

    private func refresh(_ task: BGTask) {
        Self.schedule()
        var completed = false
        let finish: @MainActor (Bool) -> Void = { success in
            guard !completed else { return }
            completed = true
            task.expirationHandler = nil
            task.setTaskCompleted(success: success)
        }
        let sync = Task {
            do {
                // A separate handle scopes cancellation; the underlying stores are shared with the UI.
                let core = try await MailModel.openCore()
                try Task.checkCancellation()
                let rows = try await withTaskCancellationHandler {
                    try await core.backgroundSync()
                } onCancel: {
                    core.cancelBackgroundSync()
                }
                try Task.checkCancellation()
                for notice in mailNotices(rows) {
                    try Task.checkCancellation()
                    let content = UNMutableNotificationContent()
                    content.title = notice.title
                    content.body = notice.body
                    content.sound = .default
                    content.threadIdentifier = notice.target.account
                    content.userInfo = ["account": notice.target.account, "message": String(notice.target.message)]
                    try await UNUserNotificationCenter.current().add(UNNotificationRequest(
                        identifier: "mail-\(notice.target.account)", content: content, trigger: nil))
                }
                finish(true)
            } catch {
                // Locked stores and credentials stay silent; the next refresh is already scheduled.
                finish(false)
            }
        }
        task.expirationHandler = {
            sync.cancel()
            Task { @MainActor in finish(false) }
        }
    }

    func requestPermission() async {
        guard UIApplication.shared.applicationState == .active, !model.accounts.isEmpty,
              !requestingPermission else { return }
        requestingPermission = true
        defer { requestingPermission = false }
        let center = UNUserNotificationCenter.current()
        guard await center.notificationSettings().authorizationStatus == .notDetermined,
              UIApplication.shared.applicationState == .active else { return }
        _ = try? await center.requestAuthorization(options: [.alert, .sound, .badge])
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                            withCompletionHandler completionHandler: @escaping () -> Void) {
        let info = response.notification.request.content.userInfo
        guard let account = info["account"] as? String, let value = info["message"] as? String,
              let message = Int64(value) else { completionHandler(); return }
        Task { @MainActor in
            model.openNotification(MailTarget(account: account, message: message))
            completionHandler()
        }
    }
}
