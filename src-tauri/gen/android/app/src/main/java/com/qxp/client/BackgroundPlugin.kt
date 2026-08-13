package com.qxp.client

import android.app.Activity
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin

/**
 * Exposes ForegroundService control to the Rust/JS side so the frontend can
 * start/stop the background keep-alive (and query its state) exactly when it
 * needs to — e.g. right after authentication.
 */
@TauriPlugin
class BackgroundPlugin(private val activity: Activity) : Plugin(activity) {

  @Command
  fun isRunning(invoke: Invoke) {
    invoke.resolveObject(ForegroundService.isRunning())
  }

  @Command
  fun start(invoke: Invoke) {
    ForegroundService.start(activity)
    invoke.resolve()
  }

  @Command
  fun stop(invoke: Invoke) {
    ForegroundService.stop(activity)
    invoke.resolve()
  }
}
