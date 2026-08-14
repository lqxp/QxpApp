package com.qxp.client

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager

/**
 * Foreground service that keeps the QxChat Android process alive while the app
 * is in the background. Android aggressively suspends WebViews and kills
 * background activities, which would tear down the JS WebSocket that receives
 * new messages and keeps long-lived calls open.
 *
 * Running a foreground service (with a persistent notification + partial wake
 * lock) tells the OS the app is doing important background work, so the WebView
 * (and its WebSocket / WebRTC) survives far longer than it otherwise would.
 */
class ForegroundService : Service() {

  companion object {
    private const val CHANNEL_ID = "qxchat-background"
    private const val NOTIFICATION_ID = 101
    private const val ACTION_START = "com.qxp.client.action.START_BACKGROUND"
    private const val ACTION_STOP = "com.qxp.client.action.STOP_BACKGROUND"

    @Volatile
    private var instance: ForegroundService? = null

    fun isRunning(): Boolean = instance != null

    fun start(context: Context) {
      val intent = Intent(context, ForegroundService::class.java).apply {
        action = ACTION_START
      }
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
        context.startForegroundService(intent)
      } else {
        context.startService(intent)
      }
    }

    fun stop(context: Context) {
      val intent = Intent(context, ForegroundService::class.java).apply {
        action = ACTION_STOP
      }
      context.startService(intent)
    }
  }

  private var wakeLock: PowerManager.WakeLock? = null

  override fun onCreate() {
    super.onCreate()
    instance = this
    createNotificationChannel()
    acquireWakeLock()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    when (intent?.action) {
      ACTION_STOP -> {
        stopSelf()
        return START_NOT_STICKY
      }
      else -> {
        startForegroundCompat()
        return START_STICKY
      }
    }
  }

  override fun onDestroy() {
    releaseWakeLock()
    instance = null
    super.onDestroy()
  }

  override fun onBind(intent: Intent?): IBinder? = null

  private fun createNotificationChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    val channel = NotificationChannel(
      CHANNEL_ID,
      getString(R.string.app_name) + " background",
      NotificationManager.IMPORTANCE_LOW,
    ).apply {
      description = "Keeps QxChat connected in the background"
      setShowBadge(false)
    }
    manager.createNotificationChannel(channel)
  }

  private fun startForegroundCompat() {
    val notification = buildNotification()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      // DATA_SYNC is a good fit for "receive new messages" background work.
      startForeground(
        NOTIFICATION_ID,
        notification,
        ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
      )
    } else {
      startForeground(NOTIFICATION_ID, notification)
    }
  }

  private fun buildNotification(): Notification {
    val launchIntent = packageManager.getLaunchIntentForPackage(packageName)
    val pendingIntent = if (launchIntent != null) {
      PendingIntent.getActivity(
        this,
        0,
        launchIntent,
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
      )
    } else {
      null
    }

    val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      Notification.Builder(this, CHANNEL_ID)
    } else {
      @Suppress("DEPRECATION")
      Notification.Builder(this)
    }

    return builder
      .setContentTitle(getString(R.string.app_name))
      .setContentText("Stay connected to receive new messages")
      .setSmallIcon(android.R.drawable.stat_notify_chat)
      .setOngoing(true)
      .setContentIntent(pendingIntent)
      .setCategory(Notification.CATEGORY_SERVICE)
      .build()
  }

  private fun acquireWakeLock() {
    if (wakeLock != null) return
    val powerManager = getSystemService(Context.POWER_SERVICE) as PowerManager
    wakeLock = powerManager.newWakeLock(
      PowerManager.PARTIAL_WAKE_LOCK,
      "qxchat::background-keepalive",
    ).apply {
      setReferenceCounted(false)
      acquire()
    }
  }

  private fun releaseWakeLock() {
    wakeLock?.let {
      if (it.isHeld) it.release()
    }
    wakeLock = null
  }
}
