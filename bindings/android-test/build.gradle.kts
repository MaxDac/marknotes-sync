// Android end-to-end tests of the published artifacts: the Kotlin bindings jar and the AAR with the Android
// libraries (built by scripts/android-libs.sh and `./gradlew assemble` in bindings/kotlin), as an app consumes them.
// The instrumented tests are the JVM tests (../kotlin/src/sharedTest) plus SmokeTest, run on an emulator by CI's
// "Android emulator (E2E)" job. `-PtestBuildType=minified` runs them against an R8-minified build, which uses only
// the keep rules shipped in the jar (META-INF/proguard/marknotes-sync.pro). See TESTING.md.
plugins {
    id("com.android.application") version "9.4.1"
    // AGP's built-in Kotlin support uses the Kotlin Gradle plugin on the classpath: the bindings' Kotlin version.
    id("org.jetbrains.kotlin.android") version "2.4.21" apply false
}

val cargoToml = file("../../Cargo.toml").readText()
val syncVersion = Regex("""\[workspace\.package][^\[]*?version\s*=\s*"([^"]+)"""").find(cargoToml)!!.groupValues[1]
// The JNA version the bindings are built and tested with.
val jnaVersion = Regex(""""net\.java\.dev\.jna:jna:([^"]+)"""").find(file("../kotlin/build.gradle.kts").readText())!!
    .groupValues[1]
val libs = file("../kotlin/build/libs")

android {
    namespace = "io.github.maxdac.marknotes.sync.e2e"
    compileSdk = 36

    defaultConfig {
        applicationId = "io.github.maxdac.marknotes.sync.e2e"
        // Matches the --platform of scripts/android-libs.sh.
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = syncVersion
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    buildTypes {
        create("minified") {
            initWith(getByName("debug"))
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
            testProguardFiles("test-rules.pro")
            matchingFallbacks += "debug"
        }
    }
    testBuildType = (findProperty("testBuildType") as String?) ?: "debug"

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets.getByName("androidTest") {
        kotlin.directories += "../kotlin/src/sharedTest/kotlin"
        assets.directories += layout.buildDirectory.dir("testAssets").get().asFile.path
    }
}

// The spec fixtures and the parity corpus, as assets under fixtures/ and parity/.
val testAssets = tasks.register<Sync>("testAssets") {
    from(file(System.getenv("MARKNOTES_SPEC_DIR") ?: "../../spec")) { include("fixtures/**/*.json") }
    from("../../target/parity") { into("parity") }
    into(layout.buildDirectory.dir("testAssets"))
}
tasks.matching { it.name.startsWith("merge") && it.name.endsWith("AndroidTestAssets") }.configureEach {
    dependsOn(testAssets)
}

dependencies {
    implementation(files(libs.resolve("marknotes-sync-kotlin-$syncVersion.jar")))
    implementation(files(libs.resolve("marknotes-sync-android-$syncVersion.aar")))
    implementation("net.java.dev.jna:jna:$jnaVersion@aar")
    androidTestImplementation("org.jetbrains.kotlin:kotlin-test-junit:2.4.21")
    androidTestImplementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.12.0")
    androidTestImplementation("androidx.test:runner:1.7.0")
    androidTestImplementation("androidx.test:core:1.7.0")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
}
