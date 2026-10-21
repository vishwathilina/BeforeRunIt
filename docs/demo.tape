# Renders docs/demo.gif with https://github.com/charmbracelet/vhs
#   vhs docs/demo.tape
# Needs beforerunit on PATH and a Nerd Font (Maple Mono NF).

Output docs/demo.gif

Set Shell "bash"
Set FontFamily "Maple Mono NF"
Set FontSize 20
Set Width 1200
Set Height 520
Set Padding 24
Set Theme "Catppuccin Mocha"
Set TypingSpeed 70ms

Hide
Type `export PS1='\[\e[1;34m\]~/project\[\e[0m\] $ '; eval "$(beforerunit init bash)"; clear`
Enter
Show

Type "git status"
Sleep 1.2s
Ctrl+U

Type "git push --force-with-lease origin main"
Sleep 1.5s
Ctrl+U

Type "chmod -R 777 public"
Sleep 1.5s
Ctrl+U

Type "rm -rf build"
Sleep 2s
Ctrl+U

Type "frobnicate --all"
Sleep 2.5s
Ctrl+U

Sleep 500ms
