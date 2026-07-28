plugins {
  alias(libs.plugins.android.application)
  alias(libs.plugins.kotlin.compose)
  alias(libs.plugins.google.devtools.ksp)
  alias(libs.plugins.roborazzi)
}

android {
  namespace = "com.omnisearch.app"
  compileSdk = 36

  val androidKeystorePath = System.getenv("ANDROID_KEYSTORE_PATH")
    ?: System.getenv("KEYSTORE_PATH")
    ?: "${rootDir}/my-upload-key.jks"
  val androidStorePassword = System.getenv("ANDROID_KEYSTORE_PASSWORD")
    ?: System.getenv("STORE_PASSWORD")
  val androidKeyAlias = System.getenv("ANDROID_KEY_ALIAS")
    ?: System.getenv("KEY_ALIAS")
    ?: "upload"
  val androidKeyPassword = System.getenv("ANDROID_KEY_PASSWORD")
    ?: System.getenv("KEY_PASSWORD")
  val hasAndroidSigningKey = file(androidKeystorePath).isFile &&
    !androidStorePassword.isNullOrBlank() &&
    androidKeyAlias.isNotBlank() &&
    !androidKeyPassword.isNullOrBlank()

  defaultConfig {
    applicationId = "com.omnisearch.app"
    minSdk = 24
    targetSdk = 35
    versionCode = 2
    versionName = "1.1"

    testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
  }

  signingConfigs {
    create("release") {
      if (hasAndroidSigningKey) {
        logger.lifecycle("✅ PRODUCTION SIGNING: Signed using '$androidKeystorePath' (Alias: '$androidKeyAlias').")
        storeFile = file(androidKeystorePath)
        storePassword = androidStorePassword
        keyAlias = androidKeyAlias
        keyPassword = androidKeyPassword
      } else {
        logger.lifecycle("⚠️ BUILD NOTICE: Production key not found, using DEBUG keystore.")
        storeFile = file("${rootDir}/debug.keystore")
        storePassword = "android"
        keyAlias = "androiddebugkey"
        keyPassword = "android"
      }
    }
  }

  buildTypes {
    release {
      isCrunchPngs = false
      isMinifyEnabled = true
      isShrinkResources = true
      proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
      signingConfig = signingConfigs.getByName("release")
    }
    create("preview") {
      initWith(getByName("release"))
      matchingFallbacks += listOf("release")
      signingConfig = if (hasAndroidSigningKey) {
        signingConfigs.getByName("release")
      } else {
        signingConfigs.getByName("debug")
      }
    }
    debug {
      // Uses Android Gradle Plugin's default debug signing key.
    }
  }
  compileOptions {
    sourceCompatibility = JavaVersion.VERSION_11
    targetCompatibility = JavaVersion.VERSION_11
  }
  buildFeatures {
    compose = true
    buildConfig = true
  }
  testOptions { unitTests { isIncludeAndroidResources = true } }
}

// Some unused dependencies are commented out below instead of being removed.
// This makes it easy to add them back in the future if needed.
dependencies {
  implementation(platform(libs.androidx.compose.bom))
  implementation(platform(libs.firebase.bom))
  // implementation(libs.accompanist.permissions)
  implementation(libs.androidx.activity.compose)
  implementation(libs.androidx.camera.camera2)
  implementation(libs.androidx.camera.core)
  implementation(libs.androidx.camera.lifecycle)
  implementation(libs.androidx.camera.view)
  implementation(libs.androidx.compose.material.icons.core)
  implementation(libs.androidx.compose.material.icons.extended)
  implementation(libs.androidx.compose.material3)
  implementation(libs.androidx.compose.ui)
  implementation(libs.androidx.compose.ui.graphics)
  implementation(libs.androidx.compose.ui.tooling.preview)
  implementation(libs.androidx.core.ktx)
  implementation(libs.androidx.datastore.preferences)
  implementation(libs.androidx.biometric)
  implementation(libs.google.mlkit.barcode.scanning)
  implementation(libs.androidx.lifecycle.runtime.compose)
  implementation(libs.androidx.lifecycle.runtime.ktx)
  implementation(libs.androidx.lifecycle.viewmodel.compose)
  implementation(libs.androidx.navigation.compose)
  implementation(libs.androidx.room.ktx)
  implementation(libs.androidx.room.runtime)
  implementation(libs.coil.compose)
  implementation(libs.coil.video)
  implementation(libs.converter.moshi)
  // implementation(libs.firebase.ai)
  implementation(libs.kotlinx.coroutines.android)
  implementation(libs.kotlinx.coroutines.core)
  implementation(libs.logging.interceptor)
  implementation(libs.moshi.kotlin)
  implementation(libs.okhttp)
  // implementation(libs.play.services.location)
  implementation(libs.retrofit)
  testImplementation(libs.androidx.compose.ui.test.junit4)
  testImplementation(libs.androidx.core)
  testImplementation(libs.androidx.junit)
  testImplementation(libs.junit)
  testImplementation(libs.kotlinx.coroutines.test)
  testImplementation(libs.robolectric)
  testImplementation(libs.roborazzi)
  testImplementation(libs.roborazzi.compose)
  testImplementation(libs.roborazzi.junit.rule)
  androidTestImplementation(platform(libs.androidx.compose.bom))
  androidTestImplementation(libs.androidx.compose.ui.test.junit4)
  androidTestImplementation(libs.androidx.espresso.core)
  androidTestImplementation(libs.androidx.junit)
  androidTestImplementation(libs.androidx.runner)
  debugImplementation(libs.androidx.compose.ui.test.manifest)
  debugImplementation(libs.androidx.compose.ui.tooling)
  "ksp"(libs.androidx.room.compiler)
  "ksp"(libs.moshi.kotlin.codegen)
}
