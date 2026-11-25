# KP Manager para Android

Consulta en el móvil, **en solo lectura**, la lista de clientes que gestionas con
[KP Manager para Windows](https://github.com/fmartineze/kpmanager). Lee el mismo fichero de datos
(`vaults.enc`) desde **OneDrive de Microsoft 365** y nunca lo modifica.

> Estado: primera versión de prueba (0.1). Abrir y navegar los vaults, la biblioteca de ficheros de clave,
> la copia de seguridad y el desbloqueo con huella llegarán en las próximas versiones.

## Qué hace esta versión

- Asistente del primer arranque:
  1. iniciar sesión con Microsoft 365;
  2. elegir la carpeta de OneDrive;
  3. localizar `vaults.enc`;
  4. escribir la contraseña maestra.
- Lista de clientes con búsqueda, etiquetas y favoritos, igual que en Windows.
- Sincronización con OneDrive. Solo descarga el fichero si cambió y, sin conexión, usa la última copia guardada.
- Se bloquea al pasar a segundo plano y tras 3 minutos sin uso.

## Seguridad

- **Solo lectura:**
  - el permiso de Microsoft es delegado y de lectura (`Files.Read`, más `User.Read`);
  - la app nunca escribe en OneDrive.
- **La configuración del móvil** (cuenta, carpeta y token de Microsoft) se guarda cifrada con tu **contraseña
  maestra**, con el mismo formato que `vaults.enc` (Argon2id + XChaCha20-Poly1305), en la carpeta privada de
  la app.
- **Inicio de sesión:** OAuth 2.0 con PKCE en el navegador del sistema, como cliente público (sin secretos en
  la app).
- **Tokens:** los tokens y las contraseñas de los vaults no salen de la parte nativa (Rust). La interfaz solo
  recibe nombres de clientes, de ficheros y de carpetas.
- **Capturas:** `FLAG_SECURE` impide que la app salga en capturas, grabaciones de pantalla y en la vista de
  apps recientes.
- **Copia automática de Android desactivada** (`allowBackup=false`).

## Registrar la app en Entra ID (una vez, como administrador del tenant)

1. Entra en <https://entra.microsoft.com> → **Aplicaciones** → **Registros de aplicaciones** →
   **Nuevo registro**.
   - Nombre: `KP Manager Android`.
   - Tipos de cuenta: **Solo las cuentas de este directorio organizativo** (un solo tenant).
   - URI de redirección: plataforma **Cliente público/nativo (móvil y escritorio)**, valor `kpmanager://auth`.
2. En **Información general**, copia el **Id. de aplicación (cliente)** y el **Id. de directorio (inquilino)**.
   La app te los pedirá en el asistente.
3. En **Permisos de API** → **Agregar un permiso** → **Microsoft Graph** → **Permisos delegados**, añade:
   - `Files.Read`
   - `User.Read`
   - `offline_access`

   Después pulsa **Conceder consentimiento de administrador**.
4. No crees secretos ni certificados: la app es un cliente público.

## Compilar

Requisitos:
- Android Studio (SDK y NDK).
- Rust con los destinos de Android.
- Node.js.

Variables de entorno: `JAVA_HOME`, `ANDROID_HOME` y `NDK_HOME`.

```
npm install
npx tauri android build --apk --target aarch64
```

El APK queda en `src-tauri/gen/android/app/build/outputs/apk/`.

> Las versiones de prueba se firman con la clave de depuración del equipo que compila. Antes de distribuirla
> de verdad hay que crear un keystore propio. Al cambiar de clave hay que desinstalar la versión anterior.

## Compatibilidad con Windows

`src-tauri/src/crypto.rs` y `model.rs` son copias de la app de Windows. El test
`reads_a_data_file_written_by_the_windows_app` abre un `vaults.enc` de prueba generado con el código de Windows
(datos ficticios): si falla, Android ha dejado de leer lo que escribe Windows.

## Licencia

MIT
