#Requires AutoHotkey v2.0

if A_Args.Length != 3 && A_Args.Length != 4 && A_Args.Length != 5
    ExitApp(2)

title := A_Args[1]
operation := A_Args[2]
MAX_HOLD_MS := 60000
MAX_WHEEL_COUNT := 20
MAX_TEXT_BYTES := 4096
MAX_DRAG_PIXELS := 512
MAX_DRAG_HOLD_MS := 5000
CLIENT_CENTER_FRACTION := 0.5

if title = ""
    ExitApp(2)

if A_Args.Length = 3 {
    key := operation
    holdMs := A_Args[3]
    if !RegExMatch(key, "^0x[0-9A-Fa-f]+$") || !RegExMatch(holdMs, "^[0-9]+$") || holdMs > MAX_HOLD_MS
        ExitApp(2)
} else if A_Args.Length = 5 && operation = "drag" {
    dx := A_Args[3]
    dy := A_Args[4]
    holdMs := A_Args[5]
    if !RegExMatch(dx, "^-?[0-9]+$") || !RegExMatch(dy, "^-?[0-9]+$") || !RegExMatch(holdMs, "^[0-9]+$")
        ExitApp(2)
    if Abs(dx) > MAX_DRAG_PIXELS || Abs(dy) > MAX_DRAG_PIXELS || (dx = 0 && dy = 0) || holdMs <= 0 || holdMs > MAX_DRAG_HOLD_MS
        ExitApp(2)
} else if A_Args.Length = 4 && operation = "wheel" {
    direction := A_Args[3]
    count := A_Args[4]
    if (direction != "up" && direction != "down") || !RegExMatch(count, "^[0-9]+$") || count < 1 || count > MAX_WHEEL_COUNT
        ExitApp(2)
} else if A_Args.Length = 4 && operation = "text" {
    textPath := A_Args[3]
    if A_Args[4] != "literal" || !FileExist(textPath) || FileGetSize(textPath) > MAX_TEXT_BYTES
        ExitApp(2)
    literal := FileRead(textPath, "UTF-8")
    if literal = "" || RegExMatch(literal, "[\x00-\x1f\x7f]")
        ExitApp(2)
} else if A_Args.Length = 4 && operation = "restore" {
    if A_Args[3] != "visible" || A_Args[4] != "only"
        ExitApp(2)
} else {
    ExitApp(2)
}

SetTitleMatchMode(2)
windows := WinGetList(title)
if windows.Length != 1
    ExitApp(3)

window := windows[1]
target := "ahk_id " window
if operation = "restore" {
    WinGetPos(&originalX, &originalY, &originalWidth, &originalHeight, target)
    try {
        WinHide(target)
    } finally {
        WinShow(target)
        WinMove(originalX, originalY, originalWidth, originalHeight, target)
    }
    WinRestore(target)
}
WinActivate(target)
if !WinWaitActive(target, , 3) || WinActive("A") != window
    ExitApp(4)

if operation = "restore" {
    ExitApp(0)
} else if A_Args.Length = 3 {
    SendEvent("{vk" SubStr(key, 3) " down}")
    Sleep(holdMs)
    SendEvent("{vk" SubStr(key, 3) " up}")
} else if operation = "wheel" || operation = "drag" {
    if GetKeyState("LButton") || GetKeyState("RButton") || GetKeyState("MButton")
        ExitApp(5)
    CoordMode("Mouse", "Screen")
    MouseGetPos(&originalX, &originalY)
    WinGetClientPos(&clientX, &clientY, &clientWidth, &clientHeight, target)
    if clientWidth <= 0 || clientHeight <= 0
        ExitApp(5)
    centerX := clientX + clientWidth * CLIENT_CENTER_FRACTION
    centerY := clientY + clientHeight * CLIENT_CENTER_FRACTION
    if operation = "drag" && (centerX + dx < clientX || centerX + dx >= clientX + clientWidth || centerY + dy < clientY || centerY + dy >= clientY + clientHeight)
        ExitApp(5)
    buttonDown := false
    try {
        MouseMove(centerX, centerY, 0)
        MouseGetPos(, , &pointerWindow)
        if WinActive("A") != window || pointerWindow != window
            throw Error("The pointer does not target the selected game window.")
        if operation = "wheel" {
            wheelKey := direction = "up" ? "WheelUp" : "WheelDown"
            SendEvent("{" wheelKey " " count "}")
        } else {
            buttonDown := true
            SendEvent("{RButton down}")
            MouseMove(centerX + dx, centerY + dy, 0)
            Sleep(holdMs)
        }
    } finally {
        if buttonDown
            SendEvent("{RButton up}")
        MouseMove(originalX, originalY, 0)
    }
} else {
    SendText(literal)
}
