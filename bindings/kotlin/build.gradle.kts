// Packages the UniFFI Kotlin bindings of the sync core:
//  - marknotes-sync-kotlin-<v>.jar: the bindings (needs JNA at runtime; works on the JVM and on Android);
//  - marknotes-sync-natives-<v>.jar: the host libraries for JVM tests, under JNA's resource prefixes;
//  - marknotes-sync-android-<v>.aar: the Android libraries (jni/<abi>/libmarknotes_sync.so).
// The generated sources (src/main/kotlin) and the native libraries (natives/) are produced by scripts/kotlin.sh.
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
    implementation("net.java.dev.jna:jna:5.17.0")
    testImplementation(kotlin("test"))
    testImplementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.11.0")
}

val natives = layout.projectDirectory.dir("natives")

tasks.jar {
    archiveBaseName.set("marknotes-sync-kotlin")
}

tasks.test {
    useJUnitPlatform()
    // The host library built by `cargo build --release --features uniffi`.
    systemProperty("jna.library.path", file("../../target/release").absolutePath)
    systemProperty("marknotes.specDir", file(System.getenv("MARKNOTES_SPEC_DIR") ?: "../../spec").absolutePath)
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
        JarOutputStream(out.resolve("classes.jar").outputStream(), Manifest()).use { }
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
