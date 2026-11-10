# Shared graphics native core

This project builds context lifecycle, bounded owned submission queues, an internal texture/FBO store, generic surface operations, shared GPU Scale2x, completed-frame upload/presentation and an ordered surface-command upload/draw/present path. The opt-in Java backend uses the JNI packet transport.

U08 added a negotiated `SCALE_SURFACE` packet operation for reusable source and destination surfaces, and the Java common adapter passed its synthetic parity gate. The U09 patched MesenCore DLL links against the same shared graphics DLL. Live Mesen validation is pending.

See [`dependencies.json`](dependencies.json) for the compiler/JDK and runtime DLL requirements, plus the pinned MesenCE revision.

## Requirements

- Windows x64 and Visual Studio C++ x64 build tools.
- CMake 3.27 or later.
- A JDK with JNI headers. No external C++ or JNI libraries are needed.
- The pinned NES source build additionally needs its matching .NET SDK; that is separate from this DLL.

U01–U08 were built with Visual Studio Build Tools 18.0, MSVC 19.50.35727 (toolset 14.50.35717), Windows SDK 10.0.26100, CMake 4.2.3-msvc3 and JDK 25.0.2. The pinned MesenCE native build used v143 (14.42.34433) with a local `/Z7` override and one MSBuild worker. The .NET 10 x64 SDK is available for the frontend build. CPU fixtures use fakes. The separate opt-in WGL harness creates a hidden WGL window and verifies synthetic upload, transforms, clipping, blending, self-copy, Scale2x, completed-frame/overlay presentation, ordered packet execution, diagnostics and caller-state behavior on the available AMD graphics adapter.

## Configure and build

From the repository root in PowerShell, set `JAVA_HOME` to an installed JDK. The following paths match the U01 environment and can be adjusted for another installation:

```powershell
$env:JAVA_HOME = 'C:\Program Files\Java\jdk-25.0.2'
$cmake = 'C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe'
& $cmake -S native-graphics -B build/native-graphics-u01 -G 'Visual Studio 18 2026' -A x64 "-DJAVA_HOME=$env:JAVA_HOME"
& $cmake --build build/native-graphics-u01 --config Release
```

`src/abi_layout_checks.cpp` compiles `sizeof` and `offsetof` assertions for every public descriptor. CMake derives the reported build ID from the ABI/protocol/native implementation inputs and compiler identity; the packaged binary still needs its separate file hash.

Run the CPU-only native fixtures with `ctest --test-dir build/native-graphics-u01 -C Release --output-on-failure`. These checks cover owned packet/frame copies, malformed packets, caps, close behavior, per-context IDs, owner checks, fake WGL sentinel fallback, pixel transfer and staged texture lifecycle. They do not prove that either emulator uses the new code.

The actual WGL pixel check is opt-in: configure with `-DRV_BUILD_GL_HARNESS=ON`, build, then run `ctest --test-dir build/native-graphics-u07 -C Debug --output-on-failure`. It returns a skipped result if a hidden compatible WGL context or required GL procedure is unavailable. The harness compares GPU results to CPU fixtures and checks completed-frame/overlay presentation, ordered command execution, aspect-fit viewport, readback bounds/formats and GL-state restoration. A passing check proves only the tested adapter and synthetic fixtures, not RTX behavior, visible gameplay, emulator integration or NES compatibility.

## Check the JNI handshake

These checks do not launch an emulator. They verify the loaded DLL's ABI/features, exact Java packet bytes, absolute-path DLL loading, shared build identity, direct-buffer JNI handoff and typed native result propagation:

```powershell
$probeClasses = 'build/native-graphics-u07/java'
New-Item -ItemType Directory -Force $probeClasses | Out-Null
& "$env:JAVA_HOME\bin\javac.exe" -d $probeClasses native-graphics/jni/java/org/revival/graphics/*.java
& "$env:JAVA_HOME\bin\java.exe" -cp $probeClasses org.revival.graphics.GraphicsPacketWriterCheck
& "$env:JAVA_HOME\bin\java.exe" --enable-native-access=ALL-UNNAMED "-Djava.library.path=$PWD\build\native-graphics-u07\bin\Debug" -cp $probeClasses org.revival.graphics.NativeGraphicsProbeCheck
& "$env:JAVA_HOME\bin\java.exe" --enable-native-access=ALL-UNNAMED -cp $probeClasses org.revival.graphics.NativeGraphicsTransportCheck "$PWD\build\native-graphics-u07\bin\Debug\revival_graphics.dll" "$PWD\build\native-graphics-u07\bin\Debug\revival_graphics_jni.dll"
```

Expected output: `Java packet writer clear/lifecycle/bounds fixtures passed.` and `ABI 1; ordered commands/frame/Scale2x/device/surface-scale features 31.` The transport check also reports successful absolute DLL loading and packet result propagation.
