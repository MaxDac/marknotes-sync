// Packages the UniFFI Kotlin bindings of the sync core:
//  - marknotes-sync-kotlin-<v>.jar: the bindings (needs JNA at runtime; works on the JVM and on Android);
//  - marknotes-sync-natives-<v>.jar: the host libraries for JVM tests, under JNA's resource prefixes;
//  - marknotes-sync-android-<v>.aar: the Android libraries (jni/<abi>/libmarknotes_sync.so).
// The generated sources (src/main/kotlin) and the native libraries (natives/) are produced by scripts/kotlin.sh.
import java.time.LocalDateTime
import java.util.jar.Attributes
import java.util.jar.JarEntry
import java.util.jar.JarOutputStream
import java.util.jar.Manifest
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    kotlin("jvm") version "2.4.21"
    kotlin("plugin.serialization") version "2.4.21"
}

val cargoToml = file("../../Cargo.toml").readText()
version = Regex("""\[workspace\.package][^\[]*?version\s*=\s*"([^"]+)"""").find(cargoToml)!!.groupValues[1]

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
        // The generated bindings trigger many warnings we can't fix here.
        suppressWarnings.set(true)
    }
}

dependencies {
    implementation("net.java.dev.jna:jna:5.19.1")
    testImplementation(kotlin("test"))
    testImplementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.12.0")
}

// The fixture tests are shared with the Android instrumented tests (bindings/android-test).
sourceSets.test { kotlin.srcDir("src/sharedTest/kotlin") }

val natives = layout.projectDirectory.dir("natives")

// Byte-for-byte reproducible archives (see docs/fdroid.md).
tasks.withType<AbstractArchiveTask>().configureEach {
    isPreserveFileTimestamps = false
    isReproducibleFileOrder = true
    filePermissions { unix("644") }
    dirPermissions { unix("755") }
}

tasks.jar {
    archiveBaseName.set("marknotes-sync-kotlin")
}

tasks.test {
    useJUnitPlatform()
    // The host library built by `cargo build --release --features uniffi`.
    systemProperty("jna.library.path", file("../../target/release").absolutePath)
    systemProperty("marknotes.specDir", file(System.getenv("MARKNOTES_SPEC_DIR") ?: "../../spec").absolutePath)
    // Root of parity/, the corpus written by `cargo test -p marknotes-sync --test parity_corpus -- --ignored`.
    systemProperty("marknotes.parityDir", file("../../target").absolutePath)
}

val nativesJar = tasks.register<Jar>("nativesJar") {
    description = "Host libraries for JVM tests (natives/jvm/<jna-prefix>/)."
    archiveBaseName.set("marknotes-sync-natives")
    from(natives.dir("jvm"))
}

val aarSkeleton = tasks.register("aarSkeleton") {
    description = "AndroidManifest.xml and an empty classes.jar for the native-only AAR."
    val dir = layout.buildDirectory.dir("aar")
    outputs.dir(dir)
    doLast {
        val out = dir.get().asFile.apply { mkdirs() }
        out.resolve("AndroidManifest.xml").writeText(
            """<manifest xmlns:android="http://schemas.android.com/apk/res/android" """ +
                """package="io.github.maxdac.marknotes.sync.natives" />""" + "\n",
        )
        // An empty jar with a fixed entry time, so the AAR is reproducible.
        JarOutputStream(out.resolve("classes.jar").outputStream()).use { jar ->
            jar.putNextEntry(
                JarEntry("META-INF/MANIFEST.MF").apply {
                    timeLocal = LocalDateTime.of(1980, 2, 1, 0, 0)
                },
            )
            Manifest().apply { mainAttributes[Attributes.Name.MANIFEST_VERSION] = "1.0" }.write(jar)
            jar.closeEntry()
        }
    }
}

val androidAar = tasks.register<Zip>("androidAar") {
    description = "Android libraries (natives/android/<abi>/libmarknotes_sync.so) as an AAR."
    archiveBaseName.set("marknotes-sync-android")
    archiveExtension.set("aar")
    destinationDirectory.set(layout.buildDirectory.dir("libs"))
    from(aarSkeleton)
    into("jni") { from(natives.dir("android")) }
}

tasks.assemble {
    dependsOn(nativesJar, androidAar)
}
