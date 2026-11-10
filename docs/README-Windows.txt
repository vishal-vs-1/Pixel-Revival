Pixel Revival - Windows x64 preview

START HERE
1. Extract the entire ZIP to a folder you can write to.
2. Open JAR Revival.exe. Keep the app and data folders beside it.
   Python, Java, and Git do not need to be installed separately.
3. In Java ME mode, click Import Java ME demo or use Add game to import a
   Java ME JAR. Use the Java ME / NES switch to open the NES library.
4. Select the game in your library, wait for its import to finish, then
   open the Play tab and click Play enhanced.

The included Revival Demo is synthetic. No commercial game JAR is included.
Fast pixel is ready immediately and works offline.

NES SETUP
The ZIP includes the shared graphics runtime. NES playback also needs the
managed MesenCE build described in docs/DEVELOPMENT.md in the source project.
Place its complete runtime tree at tools\nes-runtime beside JAR Revival.exe,
or set J2ME_REVIVAL_NES_RUNTIME_ROOT to its absolute directory before launch.
Keep the runtime's common and nes folders together. Add a .nes cartridge in
NES mode and use Play original or Play enhanced. Enhanced records tiles
during play and prepares replacements after you close the player.

KEYBOARD PROFILES
1. Open Settings to edit up to three profiles for the active system.
   Java ME and NES have separate slots. Use Capture or the key picker.
2. Select a game, open its Play tab, and choose Default or a saved profile
   from the Control profile list.
3. Each game remembers its own profile choice. Editing a shared slot applies
   to every game using it the next time those games start.

Java ME movement, fire and softkeys must stay assigned. Its keypad actions
can be unbound. NES profiles cover directions, A, B, Start and Select.
Default uses the selected system's built-in bindings.

GRAPHICS RENDERER
Java ME GPU playback defaults to Shared GPU · common-v1. Select a game and
open its GPUs tab to choose Legacy GPU · compatibility instead. This is
separate from the physical GPU selector. CPU · compatibility remains a
per-game choice in the Compatibility tab. If the shared renderer cannot
start, the player continues with Legacy GPU and identifies that fallback in
the performance label and saved player log. No second game process is started.

NES Enhanced uses the same shared renderer when a compatible managed NES
runtime is installed. NES Original keeps Mesen's stock renderer for
comparison. Enhanced artwork may look different without looking better.

OPTIONAL AI CARTOON
The AI runtime and model are not included in this ZIP.
1. Connect to the internet and open the Artwork tab.
2. Click AI setup. Wait for the status at the bottom of the window to say
   Local AI is installed. This downloads the official free Real-ESRGAN
   runtime and models once into data\tools\ai in this folder.
3. Choose AI cartoon in the Artwork style selector.
4. For a game already in your library, select it and click Prepare.
   For a new game, choose AI cartoon before Add game or Import demo.
5. Open the Play tab and click Play enhanced.

AI requires a working Vulkan-capable graphics driver. After setup, AI runs
locally and works offline. Your games and artwork are not uploaded.
If the download fails, check the connection and click AI setup again.

YOUR FILES
app contains the bundled runtimes and support files. Keep it with the EXE.
data contains your library, saves, settings, artwork cache, and downloaded AI.
Keep data when upgrading. The app creates its contents as you use it.

UPGRADING FROM THE EARLIER PREVIEW
Close the old app. Copy its library and rendered-games folders into this
version's data folder. If AI was already installed, copy the old tools\ai
folder into data\tools\ai. Keep the old copy until your games and saves
are visible in this version.

NOTICES
Project license: app\LICENSE
Python, NumPy, Pillow notices: app\THIRD-PARTY-LICENSES
Java notices: app\tools\runtime\legal
Graphics notices: app\tools\gpu
Shared graphics runtime and source: app\tools\common-runtime
FreeJ2ME notice: inside app\build\jme-player.jar

This is an experimental preview. Compatibility and artwork coverage vary
by game. Import only game files you are allowed to use.
