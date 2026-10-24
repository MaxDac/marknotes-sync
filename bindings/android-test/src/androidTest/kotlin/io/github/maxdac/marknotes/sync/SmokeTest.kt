package io.github.maxdac.marknotes.sync

import android.os.Build
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/** Checks the library the other tests run is the one packaged in this APK from the AAR, for this device's ABI. */
class SmokeTest {
    @Test
    fun theLibraryComesFromTheApk() {
        assertEquals(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            sha256Hex(ByteArray(0)),
        )
        val app = InstrumentationRegistry.getInstrumentation().targetContext.applicationInfo
        val mappings = File("/proc/self/maps").readLines().filter { it.endsWith("libmarknotes_sync.so") || "base.apk" in it }
        val core = mappings.filter { "libmarknotes_sync.so" in it }
        // Loaded straight from the APK (extractNativeLibs=false) or from the app's extracted native library dir.
        val fromApk = mappings.any { app.sourceDir in it } || core.any { app.nativeLibraryDir in it }
        assertTrue(fromApk, "libmarknotes_sync.so is not mapped from ${app.sourceDir}:\n${mappings.joinToString("\n")}")
        assertTrue(core.none { "/data/local/tmp" in it || "/system/" in it }, core.joinToString("\n"))
    }

    @Test
    fun theDeviceRunsAShippedAbi() {
        assertTrue(Build.SUPPORTED_ABIS.first() in setOf("arm64-v8a", "x86_64"), Build.SUPPORTED_ABIS.joinToString())
    }
}
