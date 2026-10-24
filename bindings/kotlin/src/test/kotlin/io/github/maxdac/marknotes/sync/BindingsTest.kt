package io.github.maxdac.marknotes.sync

import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** Behaviour of the Kotlin bindings that the spec fixtures do not cover: errors, extreme values and threads. */
class BindingsTest {
    @Test
    fun schedulerRejectsNegativeMaxPasses() {
        val e = assertFailsWith<SyncException.InvalidArgument> { SyncScheduler(listOf("a"), -1) }
        assertEquals("maxPasses must not be negative: -1", e.message)
    }

    @Test
    fun rejectedReportRecordsNothing() {
        SyncScheduler(listOf("a", "b"), 3).use { s ->
            assertEquals("a", s.next())
            assertFailsWith<SyncException.InvalidArgument> { s.report("a", -1, false) }
            assertEquals("b", s.next())
            s.report("b", 0, false)
            assertNull(s.next())
        }
    }

    @Test
    fun zeroPassesSyncsNothing() {
        SyncScheduler(listOf("a"), 0).use { assertNull(it.next()) }
    }

    @Test
    fun extremeTimesSurviveTheBoundary() {
        for (millis in listOf(Long.MIN_VALUE, -1L, 0L, Long.MAX_VALUE)) {
            val remote = RemoteEntry("a.md", "r", false, "v", millis)
            val actions = plan(SyncInput(emptyList(), listOf(remote), emptyList(), null)).actions
            assertEquals(listOf<Action>(Action.Download("a.md", remote)), actions)
        }
    }

    @Test
    fun nonAsciiAndControlCharactersSurviveTheBoundary() {
        assertEquals("ſ (conflicted copy) 2.md", conflictCopyName("ſ.md", listOf("s (conflicted copy).md")))
        assertEquals(false, isSyncablePath("a\u0000b.md"))
        assertEquals("\uFEFF---\nid: m4n5p6q7\n---\nBody", withId("\uFEFFBody", "m4n5p6q7"))
        assertEquals("😀".repeat(3), merge3(null, "😀".repeat(3), "😀".repeat(3)).text)
    }

    @Test
    fun largeInputsSurviveTheBoundary() {
        val text = (0 until 50_000).joinToString("") { "line $it\n" }
        assertEquals(text, merge3(text, text, text).text)
        assertEquals(64, sha256Hex(ByteArray(8 * 1024 * 1024)).length)
    }

    @Test
    fun schedulerIsSafeToShareAcrossThreads() {
        val drives = (0 until 8).map { "d$it" }
        SyncScheduler(drives, 3).use { s ->
            val pool = Executors.newFixedThreadPool(8)
            val seen = java.util.concurrent.ConcurrentLinkedQueue<String>()
            repeat(8) {
                pool.execute {
                    while (true) {
                        val id = s.next() ?: break
                        seen += id
                        s.report(id, 0, false)
                    }
                }
            }
            pool.shutdown()
            assertTrue(pool.awaitTermination(30, TimeUnit.SECONDS))
            assertEquals(drives.sorted(), seen.sorted())
        }
    }

    @Test
    fun pureFunctionsAreSafeToCallConcurrently() {
        val pool = Executors.newFixedThreadPool(8)
        val futures = (0 until 200).map { n ->
            pool.submit<String> { merge3("a\nb\nc\nd\ne\n", "A$n\nb\nc\nd\ne\n", "a\nb\nc\nd\nE\n").text }
        }
        futures.forEachIndexed { n, f -> assertEquals("A$n\nb\nc\nd\nE\n", f.get(30, TimeUnit.SECONDS)) }
        pool.shutdown()
    }
}
