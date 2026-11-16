package com.allternit.app.files

import android.app.job.JobParameters
import android.app.job.JobService
import kotlin.concurrent.thread

/**
 * Retries queued uploads when the network is back (JobScheduler, framework only: no
 * WorkManager dependency). [FilesDrive.scheduleUploadJob] schedules it when an upload
 * failed for a network reason; a chunked upload resumes its session (SPEC section 5).
 */
class FilesUploadJobService : JobService() {
    @Volatile private var worker: Thread? = null

    override fun onStartJob(params: JobParameters): Boolean {
        worker = thread(name = "files-upload-job") {
            val again = try {
                FilesDrive.get(this).runUploads()
            } catch (e: Exception) {
                true
            }
            jobFinished(params, again)
        }
        return true
    }

    override fun onStopJob(params: JobParameters): Boolean {
        worker?.interrupt()
        return true
    }
}
