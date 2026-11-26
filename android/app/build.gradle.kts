import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Optional release signing: drop a keystore.properties next to this file
// (storeFile, storePassword, keyAlias, keyPassword). Without it, release
// builds are signed with the debug key, which is fine for local testing.
val ksPropsFile = rootProject.file("keystore.properties")
val ksProps = Properties().apply {
    if (ksPropsFile.exists()) ksPropsFile.inputStream().use { load(it) }
}

android {
    namespace = "com.vbs.clipbridge"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.vbs.clipbridge"
        minSdk = 26
        targetSdk = 34
        versionCode = 1
        versionName = "1.0"
    }

    signingConfigs {
        if (ksPropsFile.exists()) {
            create("release") {
                storeFile = rootProject.file(ksProps.getProperty("storeFile"))
                storePassword = ksProps.getProperty("storePassword")
                keyAlias = ksProps.getProperty("keyAlias")
                keyPassword = ksProps.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            signingConfig = if (ksPropsFile.exists()) {
                signingConfigs.getByName("release")
            } else {
                signingConfigs.getByName("debug")
            }
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }

    splits {
        abi {
            isEnable = true
            reset()
            include("arm64-v8a", "x86_64")
            isUniversalApk = false
        }
    }
}
