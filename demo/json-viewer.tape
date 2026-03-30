# mdterm JSON Viewer Demo
# Demonstrates tree (card explorer) and graph views

Output demo/json-viewer.gif
Set Shell zsh
Set FontSize 14
Set Width 1200
Set Height 700
Set Padding 10
Set Theme "Catppuccin Mocha"
Set TypingSpeed 50ms

# Launch mdterm with the test JSON file
Type "cargo run --release -- tests/fixtures/test.json"
Enter
Sleep 2s

# --- Card Explorer View (Tree) ---

# Navigate down through the JSON structure
Sleep 1s
Down
Sleep 500ms
Down
Sleep 500ms
Down
Sleep 500ms

# Expand the "features" object
Enter
Sleep 1s

# Navigate into features children
Down
Sleep 500ms

# Expand "rendering"
Enter
Sleep 1s

# Navigate down to see the content
Down
Sleep 500ms
Down
Sleep 500ms
Down
Sleep 500ms

# Go back up and expand "navigation"
Up
Sleep 300ms
Up
Sleep 300ms
Up
Sleep 300ms
Up
Sleep 300ms

# Move to navigation
Down
Sleep 500ms

# Expand navigation
Enter
Sleep 1s

# Navigate down through navigation items
Down
Sleep 400ms
Down
Sleep 400ms
Down
Sleep 400ms

# Collapse navigation
Up
Sleep 300ms
Up
Sleep 300ms
Up
Sleep 300ms
Enter
Sleep 800ms

# Expand all with L
Type "L"
Sleep 1.5s

# Scroll down to see expanded content
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 300ms

Sleep 1s

# Collapse all with H
Type "H"
Sleep 1.5s

# --- Switch to Graph View ---
Sleep 500ms

# Press D to toggle to graph view
Type "D"
Sleep 2s

# Navigate down in graph view
Down
Sleep 500ms
Down
Sleep 500ms
Down
Sleep 500ms

# Expand into child card with Right/l
Type "l"
Sleep 1s

# Navigate within the child card
Down
Sleep 500ms
Down
Sleep 500ms

# Go deeper into nested structure
Type "l"
Sleep 1s

# Navigate in deeper card
Down
Sleep 400ms
Down
Sleep 400ms

# Go back to parent with h
Type "h"
Sleep 800ms

# Go back again
Type "h"
Sleep 800ms

# Move down to another top-level key
Down
Sleep 500ms
Down
Sleep 500ms

# Expand it
Type "l"
Sleep 1s

# Navigate within
Down
Sleep 400ms
Down
Sleep 400ms

# Expand all to see full graph
Type "L"
Sleep 2s

# Scroll around to see the graph layout
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 300ms

Sleep 1.5s

# Collapse all
Type "H"
Sleep 1.5s

# Switch back to card explorer
Type "D"
Sleep 1.5s

# Exit
Type "q"
Sleep 500ms
