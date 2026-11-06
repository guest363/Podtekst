# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile

# Rust вызывает эти методы MainActivity по имени через JNI. Без keep R8
# вырезает их из release-APK, и старт записи падает с NoSuchMethodError.
-keepclassmembers class io.github.guest363.podtekst.MainActivity {
    public void startRecordingService();
    public void startProcessingService();
    public void stopRecordingService();
    public void stopProcessingService();
    public void shareFile(java.lang.String);
}