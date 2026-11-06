package io.github.guest363.podtekst

import android.app.Notification
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
 *
 * Запись и расшифровка могут идти одновременно: типы foreground service
 * складываются, сервис останавливается только после завершения обеих задач.
 * Иначе смена типа на `dataSync` во время записи отбирала у неё микрофон.
 */
class RecordingService : Service() {
  companion object {
    private const val CHANNEL_ID = "podtekst_work"
    private const val NOTIFICATION_ID = 1001

    private var recording = false
    private var processing = false

    @JvmStatic
    fun startRecording(context: Context) {
      recording = true
      start(context)
    }

    @JvmStatic
    fun startProcessing(context: Context) {
      processing = true
      start(context)
    }

    @JvmStatic
    fun stopRecording(context: Context) {
      recording = false
      stopIfIdle(context)
    }

    @JvmStatic
    fun stopProcessing(context: Context) {
      processing = false
      stopIfIdle(context)
    }

    private fun stopIfIdle(context: Context) {
      if (recording || processing) {
        runCatching {
          val manager =
            context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
          manager.notify(NOTIFICATION_ID, notification(context))
        }.onFailure { Log.w("Podtekst", "update foreground notification: $it") }
      } else {
        runCatching {
          context.stopService(Intent(context, RecordingService::class.java))
        }.onFailure { Log.w("Podtekst", "stop foreground service: $it") }
      }
    }

    private fun start(context: Context) {
      val intent = Intent(context, RecordingService::class.java)
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

    private fun notification(context: Context): Notification {
      val both = recording && processing
      val title = when {
        both -> "Идёт запись и расшифровка"
        recording -> "Идёт запись"
        else -> "Идёт расшифровка"
      }
      val text = when {
        both -> "Можно погасить экран — запись и расшифровка продолжатся"
        recording -> "Можно погасить экран — запись продолжится"
        else -> "Можно свернуть приложение — обработка продолжится"
      }
      val openIntent = Intent(context, MainActivity::class.java).apply {
        addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
      }
      val pending = PendingIntent.getActivity(
        context,
        0,
        openIntent,
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
      )
      return NotificationCompat.Builder(context, CHANNEL_ID)
        .setContentTitle(title)
        .setContentText(text)
        .setSmallIcon(android.R.drawable.ic_btn_speak_now)
        .setContentIntent(pending)
        .setOngoing(true)
        .setOnlyAlertOnce(true)
        .setPriority(NotificationCompat.PRIORITY_LOW)
        .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
        .build()
    }

    private fun foregroundType(): Int {
      var type = 0
      if (recording) type = type or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
      if (processing) type = type or ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
      return if (type == 0) ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC else type
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
    if (!recording && !processing) {
      stopSelf(startId)
      return START_NOT_STICKY
    }
    if (Build.VERSION.SDK_INT >= 29) {
      startForeground(NOTIFICATION_ID, notification(this), foregroundType())
    } else {
      startForeground(NOTIFICATION_ID, notification(this))
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
