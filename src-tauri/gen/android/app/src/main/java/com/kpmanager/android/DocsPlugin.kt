package com.kpmanager.android

import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PersistableBundle
import android.provider.OpenableColumns
import android.util.Base64
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.ByteArrayOutputStream

@InvokeArg
class OpenArgs {
  var maxBytes: Int = 1024 * 1024
}

@InvokeArg
class CreateArgs {
  lateinit var fileName: String
  lateinit var mimeType: String
  lateinit var data: String
}

@InvokeArg
class CopyArgs {
  lateinit var text: String
  var clearAfterMs: Long = 30_000
}

@InvokeArg
class ShareArgs {
  lateinit var text: String
  lateinit var title: String
}

private const val CLIP_LABEL = "KP Manager"

/**
 * Abrir y guardar documentos con el selector del sistema (Storage Access Framework).
 *
 * Solo lo llama la parte nativa (Rust): la interfaz web no tiene permiso para este plugin, así que el contenido
 * de un fichero de clave o de una copia de seguridad nunca pasa por el WebView. No pide permisos de
 * almacenamiento: el usuario elige cada fichero y la app solo recibe acceso a ese.
 *
 * También copia al portapapeles: el valor llega desde Rust (nunca desde la interfaz), se marca como sensible
 * (Android no lo enseña en la vista previa) y se borra pasado un rato o al bloquear, salvo que el usuario haya
 * copiado otra cosa después.
 */
@TauriPlugin
class DocsPlugin(private val activity: Activity) : Plugin(activity) {
  private var pendingCreate: CreateArgs? = null
  private var pendingMax: Int = 0

  private val main = Handler(Looper.getMainLooper())
  /** Cuenta las copias: el borrado programado solo actúa sobre la última. */
  private var copies = 0
  /** Lo que hay en el portapapeles sigue siendo lo último que copió la app. */
  private var ours = false
  private val clipboard get() = activity.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
  private val onClipChanged = ClipboardManager.OnPrimaryClipChangedListener {
    // Con la app en primer plano se puede leer la etiqueta; si es de otra app, ya no es nuestro.
    val label = try { clipboard.primaryClipDescription?.label?.toString() } catch (e: Exception) { null }
    if (label != null && label != CLIP_LABEL) ours = false
  }

  @Command
  fun copyText(invoke: Invoke) {
    val args = invoke.parseArgs(CopyArgs::class.java)
    main.post {
      try {
        val clip = ClipData.newPlainText(CLIP_LABEL, args.text)
        clip.description.extras = PersistableBundle().apply {
          // ClipDescription.EXTRA_IS_SENSITIVE (Android 13); en versiones anteriores se ignora.
          putBoolean("android.content.extra.IS_SENSITIVE", true)
        }
        val cm = clipboard
        cm.removePrimaryClipChangedListener(onClipChanged)
        cm.setPrimaryClip(clip)
        ours = true
        val mine = ++copies
        main.post { cm.addPrimaryClipChangedListener(onClipChanged) }
        main.postDelayed({ if (mine == copies) clearIfOurs() }, args.clearAfterMs)
        invoke.resolve()
      } catch (ex: Exception) {
        invoke.reject("failed")
      }
    }
  }

  /** Menú Compartir del sistema con un texto que llega desde Rust. */
  @Command
  fun shareText(invoke: Invoke) {
    val args = invoke.parseArgs(ShareArgs::class.java)
    main.post {
      try {
        val send = Intent(Intent.ACTION_SEND).apply {
          type = "text/plain"
          putExtra(Intent.EXTRA_TEXT, args.text)
        }
        activity.startActivity(Intent.createChooser(send, args.title))
        invoke.resolve()
      } catch (ex: Exception) {
        invoke.reject("failed")
      }
    }
  }

  /** Al bloquear: borra lo copiado si sigue en el portapapeles. */
  @Command
  fun clearClipboard(invoke: Invoke) {
    main.post {
      copies++
      clearIfOurs()
      invoke.resolve()
    }
  }

  private fun clearIfOurs() {
    if (!ours) return
    val cm = clipboard
    // En segundo plano Android no deja leer el portapapeles (null): se confía en el aviso de cambios.
    val label = try { cm.primaryClipDescription?.label?.toString() } catch (e: Exception) { null }
    if (label == null || label == CLIP_LABEL) {
      try {
        if (Build.VERSION.SDK_INT >= 28) cm.clearPrimaryClip() else cm.setPrimaryClip(ClipData.newPlainText("", ""))
      } catch (e: Exception) {
      }
    }
    ours = false
    cm.removePrimaryClipChangedListener(onClipChanged)
  }

  @Command
  fun openDocument(invoke: Invoke) {
    try {
      pendingMax = invoke.parseArgs(OpenArgs::class.java).maxBytes
      val intent = Intent(Intent.ACTION_OPEN_DOCUMENT)
      intent.addCategory(Intent.CATEGORY_OPENABLE)
      intent.type = "*/*"
      startActivityForResult(invoke, intent, "openResult")
    } catch (ex: Exception) {
      invoke.reject("failed")
    }
  }

  @ActivityCallback
  fun openResult(invoke: Invoke, result: ActivityResult) {
    val uri = result.data?.data
    if (result.resultCode != Activity.RESULT_OK || uri == null) {
      invoke.reject("cancelled")
      return
    }
    try {
      val resolver = activity.contentResolver
      var name = "fichero"
      var size = -1L
      resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
        if (c.moveToFirst()) {
          val n = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
          val s = c.getColumnIndex(OpenableColumns.SIZE)
          if (n >= 0 && !c.isNull(n)) name = c.getString(n)
          if (s >= 0 && !c.isNull(s)) size = c.getLong(s)
        }
      }
      if (size > pendingMax) {
        invoke.reject("tooLarge")
        return
      }
      // Se lee con límite aunque el proveedor no diga el tamaño (o mienta).
      val out = ByteArrayOutputStream()
      val input = resolver.openInputStream(uri)
      if (input == null) {
        invoke.reject("failed")
        return
      }
      input.use { s ->
        val buf = ByteArray(16 * 1024)
        while (true) {
          val r = s.read(buf)
          if (r < 0) break
          if (out.size() + r > pendingMax) {
            invoke.reject("tooLarge")
            return
          }
          out.write(buf, 0, r)
        }
      }
      val bytes = out.toByteArray()
      val ret = JSObject()
      ret.put("name", name)
      ret.put("data", Base64.encodeToString(bytes, Base64.NO_WRAP))
      bytes.fill(0)
      invoke.resolve(ret)
    } catch (ex: Exception) {
      invoke.reject("failed")
    }
  }

  @Command
  fun createDocument(invoke: Invoke) {
    try {
      val args = invoke.parseArgs(CreateArgs::class.java)
      pendingCreate = args
      val intent = Intent(Intent.ACTION_CREATE_DOCUMENT)
      intent.addCategory(Intent.CATEGORY_OPENABLE)
      intent.type = args.mimeType
      intent.putExtra(Intent.EXTRA_TITLE, args.fileName)
      startActivityForResult(invoke, intent, "createResult")
    } catch (ex: Exception) {
      pendingCreate = null
      invoke.reject("failed")
    }
  }

  @ActivityCallback
  fun createResult(invoke: Invoke, result: ActivityResult) {
    val args = pendingCreate
    pendingCreate = null
    val uri: Uri? = result.data?.data
    if (result.resultCode != Activity.RESULT_OK || uri == null || args == null) {
      invoke.reject("cancelled")
      return
    }
    try {
      val bytes = Base64.decode(args.data, Base64.NO_WRAP)
      val out = activity.contentResolver.openOutputStream(uri, "wt")
      if (out == null) {
        invoke.reject("failed")
        return
      }
      out.use { it.write(bytes) }
      var name = args.fileName
      activity.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
        if (c.moveToFirst() && !c.isNull(0)) name = c.getString(0)
      }
      val ret = JSObject()
      ret.put("name", name)
      invoke.resolve(ret)
    } catch (ex: Exception) {
      invoke.reject("failed")
    }
  }
}
