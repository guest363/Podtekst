package io.github.guest363.podtekst

import android.Manifest
import android.content.pm.ActivityInfo
import android.os.Build
import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

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
    RecordingService.stop(this)
  }
}
