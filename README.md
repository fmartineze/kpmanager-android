# KP Manager para Android

Consulta en el móvil, **en solo lectura**, la lista de clientes que gestionas con
[KP Manager para Windows](https://github.com/fmartineze/kpmanager). Lee el mismo fichero de datos
(`vaults.enc`) desde **OneDrive de Microsoft 365** y nunca lo modifica.

> Estado: versión de prueba (0.3).

## Qué hace esta versión

- Asistente del primer arranque:
  1. iniciar sesión con Microsoft 365;
  2. elegir la carpeta de OneDrive;
  3. localizar `vaults.enc`;
  4. escribir la contraseña maestra.

  O bien **restaurar una copia de seguridad** de otro móvil: se elige el fichero, se escribe la maestra y se
  inicia sesión con Microsoft.
- **Biblioteca de ficheros de clave**, solo en el móvil (no se sincronizan):
  - se importan con el selector de ficheros de Android;
  - cada vault usa automáticamente el fichero que se llama igual que en Windows o, si no hay ninguno, el
    predeterminado;
  - en la ficha de cada cliente se puede elegir otro o ninguno.
- **Navegar los vaults en solo lectura** (tocar un cliente):
  - grupos, búsqueda, ficha de cada entrada, código TOTP, notas en Markdown y adjuntos;
  - el `.kdbx` se localiza en OneDrive a partir de su ruta de Windows y se guarda una copia en el móvil para
    abrirlo sin conexión (solo se vuelve a descargar si cambió).
- **Desbloqueo con huella** (Ajustes).
- **Copia de seguridad** (Ajustes): cuenta, carpeta, fichero de datos y ficheros de clave, en un fichero
  `.kpmbak` cifrado con la contraseña maestra.
- Lista de clientes con búsqueda, etiquetas y favoritos, igual que en Windows.
- Sincronización con OneDrive. Solo descarga el fichero si cambió y, sin conexión, usa la última copia guardada.
- Se bloquea tras 30 segundos en segundo plano (tiempo para pegar lo copiado en otra app) y tras 3 minutos
  sin uso.

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
- **Ficheros de clave:** se guardan dentro de la configuración cifrada. Se leen con el selector del sistema
  directamente desde Rust (plugin propio `DocsPlugin`, que la interfaz web no puede llamar): su contenido nunca
  pasa por el WebView. La app no pide permisos de almacenamiento; solo accede al fichero que eliges.
- **Vaults:** el `.kdbx` se trata como hostil (mismo código que en Windows): tamaño y parámetros de la
  derivación de clave acotados, iconos validados y notas saneadas. A la interfaz solo llega el árbol sin
  contraseñas, notas ni semillas TOTP; cada secreto se pide al mostrarlo. La app nunca escribe en el vault.
- **Portapapeles:** se copia desde Rust (el valor no pasa por la interfaz), marcado como sensible para que
  Android no lo enseñe en la vista previa, y se borra a los 30 segundos (salvo que hayas copiado otra cosa
  después) o al bloquear con el botón.
- **Enlaces:** solo `https`, `http` y `mailto`, y siempre se enseña la dirección real antes de abrirla.
- **Huella:** la maestra se cifra con una clave AES del almacén de claves de Android que exige huella de
  seguridad alta en cada uso y que se invalida si se añade una huella nueva. La maestra no pasa por la interfaz.
- **Copia de seguridad:** cifrada con la maestra (Argon2id + XChaCha20-Poly1305). No incluye el token de
  Microsoft: en el móvil nuevo hay que volver a iniciar sesión. Guárdala en un sitio de confianza: con la
  maestra da acceso a tus ficheros de clave.
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
