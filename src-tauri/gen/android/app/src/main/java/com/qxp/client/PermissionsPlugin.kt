package com.qxp.client

import android.Manifest
import android.app.Activity
import app.tauri.annotation.Permission
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Plugin

/**
 * Native Android runtime-permission gateway for the QxChat client.
 *
 * The web view does not reliably trigger system permission dialogs (camera,
 * microphone, notifications, media/storage) on its own. This plugin exposes the
 * standard `requestPermissions` / `checkPermissions` commands (inherited from
 * [Plugin]) backed by the `@TauriPlugin` permission declarations below, so the
 * frontend can request every feature it needs right after authentication /
 * unlock instead of relying on the web runtime.
 */
@TauriPlugin(
  permissions = [
    Permission(strings = [Manifest.permission.CAMERA], alias = "camera"),
    Permission(strings = [Manifest.permission.RECORD_AUDIO], alias = "microphone"),
    Permission(strings = [Manifest.permission.POST_NOTIFICATIONS], alias = "notifications"),
    Permission(
      strings = [
        Manifest.permission.READ_EXTERNAL_STORAGE,
        Manifest.permission.READ_MEDIA_IMAGES,
        Manifest.permission.READ_MEDIA_VIDEO,
        Manifest.permission.READ_MEDIA_AUDIO,
      ],
      alias = "storage",
    ),
  ]
)
class PermissionsPlugin(private val activity: Activity) : Plugin(activity)
