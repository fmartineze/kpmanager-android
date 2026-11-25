package com.kpmanager.android

import android.app.Activity
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

@InvokeArg
class BioArgs {
  lateinit var data: String
  lateinit var title: String
  lateinit var cancel: String
}

private const val KEY_ALIAS = "kpmanager-bio"
private const val GCM_IV = 12

/**
 * Desbloqueo con huella. La contraseña maestra se cifra con una clave AES del almacén de claves de Android
 * que solo se puede usar tras una autenticación biométrica fuerte (cada vez) y que el sistema invalida si
 * se añade una huella nueva. La app guarda solo el resultado cifrado.
 *
 * Como `DocsPlugin`, solo lo llama Rust: la maestra nunca pasa por la interfaz web.
 */
@TauriPlugin
class BioPlugin(private val activity: Activity) : Plugin(activity) {

  private fun keyStore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

  private fun newKey(): SecretKey {
    keyStore().deleteEntry(KEY_ALIAS)
    val spec = KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
      .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
      .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
      .setKeySize(256)
      .setUserAuthenticationRequired(true)
      .setInvalidatedByBiometricEnrollment(true)
      .apply {
        if (Build.VERSION.SDK_INT >= 30) setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
      }
      .build()
    val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
    gen.init(spec)
    return gen.generateKey()
  }

  private fun existingKey(): SecretKey? = keyStore().getKey(KEY_ALIAS, null) as SecretKey?

  @Command
  fun status(invoke: Invoke) {
    val ret = JSObject()
    ret.put("available", BiometricManager.from(activity).canAuthenticate(BIOMETRIC_STRONG) == BiometricManager.BIOMETRIC_SUCCESS)
    invoke.resolve(ret)
  }

  /** Cifra `data` (base64) tras pedir la huella. Crea una clave nueva cada vez. */
  @Command
  fun enroll(invoke: Invoke) {
    val args = invoke.parseArgs(BioArgs::class.java)
    val cipher = try {
      Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, newKey()) }
    } catch (ex: Exception) {
      invoke.reject("unavailable")
      return
    }
    prompt(invoke, args, cipher) { c ->
      val plain = Base64.decode(args.data, Base64.NO_WRAP)
      val sealed = c.iv + c.doFinal(plain)
      plain.fill(0)
      Base64.encodeToString(sealed, Base64.NO_WRAP)
    }
  }

  /** Descifra `data` (base64, IV + cifrado) tras pedir la huella. */
  @Command
  fun decrypt(invoke: Invoke) {
    val args = invoke.parseArgs(BioArgs::class.java)
    val sealed = Base64.decode(args.data, Base64.NO_WRAP)
    if (sealed.size <= GCM_IV) {
      invoke.reject("invalidated")
      return
    }
    val cipher = try {
      val key = existingKey() ?: run {
        invoke.reject("invalidated")
        return
      }
      Cipher.getInstance("AES/GCM/NoPadding").apply {
        init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, sealed, 0, GCM_IV))
      }
    } catch (ex: KeyPermanentlyInvalidatedException) {
      // Se añadió o quitó una huella: la clave ya no sirve.
      keyStore().deleteEntry(KEY_ALIAS)
      invoke.reject("invalidated")
      return
    } catch (ex: Exception) {
      invoke.reject("unavailable")
      return
    }
    prompt(invoke, args, cipher) { c ->
      val plain = c.doFinal(sealed, GCM_IV, sealed.size - GCM_IV)
      val out = Base64.encodeToString(plain, Base64.NO_WRAP)
      plain.fill(0)
      out
    }
  }

  @Command
  fun remove(invoke: Invoke) {
    try {
      keyStore().deleteEntry(KEY_ALIAS)
    } catch (ex: Exception) {
    }
    invoke.resolve()
  }

  private fun prompt(invoke: Invoke, args: BioArgs, cipher: Cipher, done: (Cipher) -> String) {
    val fragment = activity as? FragmentActivity ?: run {
      invoke.reject("unavailable")
      return
    }
    activity.runOnUiThread {
      val callback = object : BiometricPrompt.AuthenticationCallback() {
        override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
          val c = result.cryptoObject?.cipher
          if (c == null) {
            invoke.reject("failed")
            return
          }
          try {
            val ret = JSObject()
            ret.put("data", done(c))
            invoke.resolve(ret)
          } catch (ex: Exception) {
            invoke.reject("failed")
          }
        }

        override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
          val cancelled = errorCode == BiometricPrompt.ERROR_USER_CANCELED ||
            errorCode == BiometricPrompt.ERROR_NEGATIVE_BUTTON ||
            errorCode == BiometricPrompt.ERROR_CANCELED
          invoke.reject(if (cancelled) "cancelled" else if (errorCode == BiometricPrompt.ERROR_LOCKOUT || errorCode == BiometricPrompt.ERROR_LOCKOUT_PERMANENT) "lockout" else "failed")
        }
        // onAuthenticationFailed: huella no reconocida; el diálogo sigue abierto y deja reintentar.
      }
      val info = BiometricPrompt.PromptInfo.Builder()
        .setTitle(args.title)
        .setNegativeButtonText(args.cancel)
        .setAllowedAuthenticators(BIOMETRIC_STRONG)
        .build()
      BiometricPrompt(fragment, ContextCompat.getMainExecutor(activity), callback)
        .authenticate(info, BiometricPrompt.CryptoObject(cipher))
    }
  }
}
