package app.lagommail.android

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat
import androidx.work.*
import app.lagommail.core.MailRow
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import java.util.concurrent.TimeUnit

internal data class MailTarget(val account: String, val message: Long)
internal data class MailNotice(val target: MailTarget, val title: String, val body: String)

internal fun mailNotices(rows: List<MailRow>): List<MailNotice> = rows.groupBy { it.account }.toSortedMap().map { (account, mail) ->
    val newest = mail.sortedWith(compareByDescending<MailRow> { it.dateUnix }.thenByDescending { it.id })
    val first = newest.first()
    MailNotice(MailTarget(account, first.id),
        if (mail.size == 1) first.senderName.ifEmpty { first.senderAddress } else "${mail.size} new messages",
        if (mail.size == 1) first.subject else newest.take(3).joinToString("\n") { it.subject })
}

class MailRefreshWorker(context: Context, parameters: WorkerParameters) : CoroutineWorker(context, parameters) {
    override suspend fun doWork(): Result = try {
        val core = withContext(Dispatchers.IO) { (applicationContext as LagomApplication).backgroundCore() }
        // Dropping the FFI future does not stop Rust's blocking worker; the flag stops it before the next account.
        val rows = try { core.backgroundSync() } catch (cancelled: CancellationException) { core.cancelBackgroundSync(); throw cancelled }
        val manager = applicationContext.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel("new-mail", "New mail", NotificationManager.IMPORTANCE_DEFAULT))
        if (manager.areNotificationsEnabled()) for (notice in mailNotices(rows)) {
            currentCoroutineContext().ensureActive()
            val intent = Intent(applicationContext, MainActivity::class.java)
                .setAction("app.lagommail.OPEN.${notice.target.account}")
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
                .putExtra("account", notice.target.account).putExtra("message", notice.target.message)
            val pending = PendingIntent.getActivity(applicationContext, 0, intent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
            val notification = NotificationCompat.Builder(applicationContext, "new-mail")
                .setSmallIcon(R.drawable.ic_notification)
                .setContentTitle(notice.title).setContentText(notice.body)
                .setStyle(NotificationCompat.BigTextStyle().bigText(notice.body))
                .setContentIntent(pending).setAutoCancel(true).build()
            manager.notify(notice.target.account, 0, notification)
        }
        Result.success()
    } catch (cancelled: CancellationException) { throw cancelled }
    catch (_: Exception) {
        // Locked stores/credentials fail quietly; periodic work remains scheduled.
        Result.success()
    }

    companion object {
        fun schedule(context: Context) {
            val request = PeriodicWorkRequestBuilder<MailRefreshWorker>(15, TimeUnit.MINUTES)
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()).build()
            WorkManager.getInstance(context).enqueueUniquePeriodicWork("lagom-refresh", ExistingPeriodicWorkPolicy.KEEP, request)
        }
    }
}

class SendWorker(context: Context, parameters: WorkerParameters) : CoroutineWorker(context, parameters) {
    override suspend fun doWork(): Result = try {
        val core = withContext(Dispatchers.IO) { (applicationContext as LagomApplication).backgroundCore() }
        core.syncAll()
        Result.success()
    } catch (cancelled: CancellationException) { throw cancelled }
    // Offline or locked credentials: a few retries, then the next app start sends.
    catch (_: Exception) { if (runAttemptCount < 3) Result.retry() else Result.success() }

    companion object {
        /** The delay covers the Undo window; an undone row is gone by then and nothing is sent. */
        fun schedule(context: Context) {
            val request = OneTimeWorkRequestBuilder<SendWorker>().setInitialDelay(5, TimeUnit.SECONDS)
                // Matches the outbox's 60 s doubling, so a retry never runs before the failed mail is due again.
                .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 60, TimeUnit.SECONDS)
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()).build()
            WorkManager.getInstance(context).enqueueUniqueWork("lagom-send", ExistingWorkPolicy.REPLACE, request)
        }
    }
}
