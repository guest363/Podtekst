package io.github.guest363.podtekst

import android.Manifest
import android.content.ClipData
import android.content.Intent
import android.content.pm.ActivityInfo
import android.os.Build
import android.os.Bundle
import android.util.Log
import androidx.activity.enableEdgeToEdge
import androidx.core.content.FileProvider
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import java.io.File

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_PORTRAIT
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)

    ViewCompat.setOnApplyWindowInsetsListener(findViewById(android.R.id.content)) { view, windowInsets ->
      val insets = windowInsets.getInsets(
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.ime()
      )
      view.setPadding(insets.left, insets.top, insets.right, insets.bottom)
      WindowInsetsCompat.CONSUMED
    }

    val needed = mutableListOf(Manifest.permission.RECORD_AUDIO)
    if (Build.VERSION.SDK_INT >= 33) {
      needed.add(Manifest.permission.POST_NOTIFICATIONS)
    }
    if (!PermissionHelper.hasPermissions(this, needed.toTypedArray())) {
      requestPermissions(needed.toTypedArray()) { _ -> }
    }
  }

  // JNI-мост из Rust: FindClass из нативной нити не видит классы приложения,
  // поэтому вызываем методы activity, а не ищем RecordingService через JNI.
  @Suppress("unused")
  fun startRecordingService() {
    RecordingService.startRecording(this)
  }

  @Suppress("unused")
  fun startProcessingService() {
    RecordingService.startProcessing(this)
  }

  @Suppress("unused")
  fun stopRecordingService() {
    RecordingService.stopRecording(this)
  }

  @Suppress("unused")
  fun stopProcessingService() {
    RecordingService.stopProcessing(this)
  }

  // Отчёт диагностики лежит во внутренней песочнице: без этой передачи файл
  // не виден ни файловому менеджеру, ни почте.
  @Suppress("unused")
  fun shareFile(path: String) {
    runCatching {
      val file = File(path)
      if (!file.isFile) return
      val uri = FileProvider.getUriForFile(this, "$packageName.fileprovider", file)
      val send = Intent(Intent.ACTION_SEND).apply {
        type = "application/zip"
        putExtra(Intent.EXTRA_STREAM, uri)
        clipData = ClipData.newRawUri("diagnostics", uri)
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
      }
      startActivity(Intent.createChooser(send, "Отправить отчёт"))
    }.onFailure {
      Log.w("Podtekst", "share diagnostics: $it")
    }
  }
}
