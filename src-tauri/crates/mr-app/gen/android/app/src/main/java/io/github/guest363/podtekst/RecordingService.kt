package io.github.guest363.podtekst

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationCompat

/**
 * Foreground service для записи и расшифровки.
 *
 * Android 14+ не даёт приложению в фоне читать микрофон: без foreground service
 * AAudio продолжает отдавать нулевые сэмплы, и запись 40-минутной встречи
 * превращается в 40 минут тишины. Сервис также держит partial wake lock, чтобы
 * расшифровка не останавливалась при погасшем экране.
 */
class RecordingService : Service() {
  companion object {
    const val ACTION_RECORDING = "io.github.guest363.podtekst.RECORDING"
    const val ACTION_PROCESSING = "io.github.guest363.podtekst.PROCESSING"
    private const val CHANNEL_ID = "podtekst_work"
    private const val NOTIFICATION_ID = 1001

    @JvmStatic
    fun startRecording(context: Context) = start(context, false)

    @JvmStatic
    fun startProcessing(context: Context) = start(context, true)

    @JvmStatic
    fun stop(context: Context) {
      runCatching {
        context.stopService(Intent(context, RecordingService::class.java))
      }.onFailure { Log.w("Podtekst", "stop foreground service: $it") }
    }

    private fun start(context: Context, processing: Boolean) {
      val intent = Intent(context, RecordingService::class.java).apply {
        action = if (processing) ACTION_PROCESSING else ACTION_RECORDING
      }
      runCatching {
        if (Build.VERSION.SDK_INT >= 26) {
          context.startForegroundService(intent)
        } else {
          context.startService(intent)
        }
      }.onFailure {
        // Из фона Android запрещает старт FGS. Не роняем приложение: запись
        // в таком состоянии всё равно не получит микрофон, но UI покажет статус.
        Log.e("Podtekst", "start foreground service: $it")
      }
    }
  }

  private var wakeLock: PowerManager.WakeLock? = null

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onCreate() {
    super.onCreate()
    createChannel()
    val pm = getSystemService(POWER_SERVICE) as PowerManager
    wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "Podtekst:work").apply {
      setReferenceCounted(false)
      acquire()
    }
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    val processing = intent?.action == ACTION_PROCESSING
    val title = if (processing) "Идёт расшифровка" else "Идёт запись"
    val text = if (processing)
      "Можно свернуть приложение — обработка продолжится"
    else
      "Можно погасить экран — запись продолжится"

    val openIntent = Intent(this, MainActivity::class.java).apply {
      addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
    }
    val pending = PendingIntent.getActivity(
      this,
      0,
      openIntent,
      PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
    )
    val notification = NotificationCompat.Builder(this, CHANNEL_ID)
      .setContentTitle(title)
      .setContentText(text)
      .setSmallIcon(android.R.drawable.ic_btn_speak_now)
      .setContentIntent(pending)
      .setOngoing(true)
      .setOnlyAlertOnce(true)
      .setPriority(NotificationCompat.PRIORITY_LOW)
      .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
      .build()

    if (Build.VERSION.SDK_INT >= 34) {
      val type = if (processing)
        ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
      else
        ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
      startForeground(NOTIFICATION_ID, notification, type)
    } else if (Build.VERSION.SDK_INT >= 30) {
      val type = if (processing)
        ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
      else
        ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
      startForeground(NOTIFICATION_ID, notification, type)
    } else if (Build.VERSION.SDK_INT >= 29) {
      startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
    } else {
      startForeground(NOTIFICATION_ID, notification)
    }
    return START_NOT_STICKY
  }

  private fun createChannel() {
    if (Build.VERSION.SDK_INT < 26) return
    val manager = getSystemService(NotificationManager::class.java)
    if (manager.getNotificationChannel(CHANNEL_ID) == null) {
      val channel = NotificationChannel(
        CHANNEL_ID,
        "Запись и расшифровка",
        NotificationManager.IMPORTANCE_LOW,
      ).apply {
        setShowBadge(false)
        description = "Фоновый статус долгой записи и обработки"
      }
      manager.createNotificationChannel(channel)
    }
  }

  override fun onDestroy() {
    wakeLock?.let { if (it.isHeld) it.release() }
    wakeLock = null
    super.onDestroy()
  }
}
