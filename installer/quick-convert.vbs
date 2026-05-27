Option Explicit

Dim shell
Dim filesystem
Dim scriptDirectory
Dim powerShellScript
Dim inputPath
Dim command

If WScript.Arguments.Count < 1 Then
    WScript.Quit 1
End If

Set shell = CreateObject("WScript.Shell")
Set filesystem = CreateObject("Scripting.FileSystemObject")

scriptDirectory = filesystem.GetParentFolderName(WScript.ScriptFullName)
powerShellScript = filesystem.BuildPath(scriptDirectory, "quick-convert.ps1")
inputPath = WScript.Arguments(0)

command = Quote("powershell.exe") & " -Sta -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File " & Quote(powerShellScript) & " " & Quote(inputPath)
shell.Run command, 0, False

Function Quote(ByVal value)
    Quote = Chr(34) & Replace(value, Chr(34), Chr(34) & Chr(34)) & Chr(34)
End Function