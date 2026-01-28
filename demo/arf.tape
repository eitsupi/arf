# arf demo - A cross-platform R console written in Rust
Output arf.gif

# Terminal settings
Set Shell bash
Set FontSize 22
Set Width 960
Set Height 600
Set Padding 20
Set TypingSpeed 75ms

# Start arf
Type "arf"
Enter
Sleep 2s

# Basic R expression
Type "1 + 1"
Enter
Sleep 1s

# Tab completion demo - type "pri", tab, select 2nd option (print)
Type "pri"
Sleep 500ms
Tab
Sleep 800ms
Down
Sleep 300ms
Enter
Sleep 300ms
# Now we have print( with auto-matched )
Type '"'
Type "Hello arf!"
Right
Right
Enter
Sleep 1s

# Switch R version
Type 'print("R version switching'
Enter
Sleep 1s
Type ":swi"
Tab
Tab
Sleep 1s
Type "4.4"
Sleep 500ms
Enter
Sleep 500ms
Enter
Sleep 1s
Type "y"
Enter
Sleep 2s

# Error
Type 'print("Error status indication'
Enter
Sleep 1s
Type 'stop("error!'
Enter
Sleep 3s

# history
Type 'print("Fuzzy history search'
Enter
Ctrl+R
Sleep 2s
Type "hl"
Sleep 2s
Enter
Sleep 1s
Enter
Sleep 4s
