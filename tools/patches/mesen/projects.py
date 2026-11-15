"""Add bridge sources and dependencies to Mesen build projects."""

from .anchors import _require


def patch_project(project: str) -> str:
    anchors = [
        '<ClCompile Include="EmuApiWrapper.cpp" />',
        '<ClInclude Include="Common.h" />',
        '<AdditionalIncludeDirectories>\n      </AdditionalIncludeDirectories>',
        '<PreprocessorDefinitions>WIN32;',
        '<AdditionalDependencies>dinput8.lib;',
    ]
    for anchor in anchors:
        if anchor == '<AdditionalIncludeDirectories>\n      </AdditionalIncludeDirectories>':
            if anchor not in project and "nes-bridge\\core;" not in project:
                _require(project, anchor)
        elif anchor == '<PreprocessorDefinitions>WIN32;':
            if anchor not in project and "REVIVAL_MESEN_BRIDGE;WIN32;" not in project:
                _require(project, anchor)
        elif anchor == '<AdditionalDependencies>dinput8.lib;':
            if anchor not in project and "$(REVIVAL_GRAPHICS_LIB);opengl32.lib;dinput8.lib;" not in project:
                _require(project, anchor)
        else:
            _require(project, anchor)
    if "RevivalRenderingDevice.hpp" not in project:
        project = project.replace(
            '<ClInclude Include="Common.h" />',
            '<ClInclude Include="Common.h" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalRenderingDevice.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalFrameContract.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserver.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\windows\\RevivalWglHost.hpp" />',
            1,
        )
    elif "NesTileObserver.hpp" not in project:
        project = project.replace(
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalFrameContract.hpp" />',
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalFrameContract.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserver.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.hpp" />',
            1,
        )
    if "NesCaptureWriter.hpp" not in project:
        project = project.replace(
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.hpp" />',
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesCaptureWriter.hpp" />',
            1,
        )
    if "NesInputMapper.hpp" not in project:
        project = project.replace(
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesCaptureWriter.hpp" />',
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesCaptureWriter.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesInputMapper.hpp" />',
            1,
        )
    if "NesTileObserver.cpp" not in project:
        if "RevivalRenderingDevice.cpp" in project:
            project = project.replace(
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalFrameContract.cpp" />',
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalFrameContract.cpp" />\n'
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserver.cpp" />\n'
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.cpp" />',
                1,
            )
        else:
            project = project.replace(
                '<ClCompile Include="EmuApiWrapper.cpp" />',
                '<ClCompile Include="EmuApiWrapper.cpp" />\n'
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalRenderingDevice.cpp" />\n'
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalFrameContract.cpp" />\n'
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserver.cpp" />\n'
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.cpp" />\n'
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\windows\\RevivalWglHost.cpp" />',
                1,
            )
    if "NesCaptureWriter.cpp" not in project:
        project = project.replace(
            '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.cpp" />',
            '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.cpp" />\n'
            '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesCaptureWriter.cpp" />',
            1,
        )
    if "NesInputMapper.cpp" not in project:
        project = project.replace(
            '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesCaptureWriter.cpp" />',
            '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesCaptureWriter.cpp" />\n'
            '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesInputMapper.cpp" />',
            1,
        )
    if "nes-bridge\\core;" not in project:
        project = project.replace(
            '<AdditionalIncludeDirectories>\n      </AdditionalIncludeDirectories>',
            '<AdditionalIncludeDirectories>$(ProjectDir)..\\..\\..\\nes-bridge\\core;$(ProjectDir)..\\..\\..\\nes-bridge\\windows;$(ProjectDir)..\\..\\..\\native-graphics\\include;%(AdditionalIncludeDirectories)\n      </AdditionalIncludeDirectories>',
            4,
        )
    if "REVIVAL_MESEN_BRIDGE;WIN32;" not in project:
        project = project.replace(
            '<PreprocessorDefinitions>WIN32;',
            '<PreprocessorDefinitions>REVIVAL_MESEN_BRIDGE;WIN32;',
        )
    if "$(REVIVAL_GRAPHICS_LIB);opengl32.lib;dinput8.lib;" not in project:
        project = project.replace(
            '<AdditionalDependencies>dinput8.lib;',
            '<AdditionalDependencies>$(REVIVAL_GRAPHICS_LIB);opengl32.lib;dinput8.lib;',
        )
    return project


def patch_core_project(project: str) -> str:
    anchor = '<AdditionalIncludeDirectories>\n      </AdditionalIncludeDirectories>'
    correct = "$(ProjectDir)..\\..\\..\\nes-bridge\\core;"
    wrong = "$(SolutionDir)..\\..\\..\\nes-bridge\\core;"
    if correct in project:
        if wrong in project:
            return project.replace(wrong, correct)
        return project
    if wrong in project:
        return project.replace(wrong, correct)
    _require(project, anchor)
    return project.replace(
        anchor,
        '<AdditionalIncludeDirectories>$(ProjectDir)..\\..\\..\\nes-bridge\\core;%(AdditionalIncludeDirectories)\n      </AdditionalIncludeDirectories>',
    )
