plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.klip.app"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.klip.app"
        minSdk = 26
        targetSdk = 34
        versionCode = 20
        versionName = "0.8.0-alpha"
    }

    // One APK per CPU type instead of one 39 MB universal file: phones get only their own native
    // library. Default = the two real phone ABIs; for the emulator build with -Pklip.abis=x86_64.
    // The ABIs must have been built by android/build-core.sh.
    val abis = (project.findProperty("klip.abis") as String?)?.split(",") ?: listOf("arm64-v8a", "armeabi-v7a")
    splits {
        abi {
            isEnable = true
            reset()
            include(*abis.toTypedArray())
            isUniversalApk = false
        }
    }

    // Release signing comes from the environment (CI secrets or a local keystore); without it the
    // release build stays unsigned and `assembleDebug` is used instead. Nothing secret lives here.
    val keystoreFile = System.getenv("KLIP_KEYSTORE_FILE")
    signingConfigs {
        if (keystoreFile != null) {
            create("release") {
                storeFile = file(keystoreFile)
                storePassword = System.getenv("KLIP_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("KLIP_KEY_ALIAS")
                keyPassword = System.getenv("KLIP_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            if (keystoreFile != null) signingConfig = signingConfigs.getByName("release")
            // For tests only: -Pklip.debuggableRelease lets `run-as` read the app's files in an R8 build.
            isDebuggable = project.hasProperty("klip.debuggableRelease")
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
}

dependencies {
    // No AppCompat / Material: the UI is plain framework views (see ui/Ui.kt). Keeps the APK small.
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
    // UniFFI-generated Kotlin bindings load the Rust library through JNA.
    implementation("net.java.dev.jna:jna:5.14.0@aar")
    testImplementation("junit:junit:4.13.2")
}
