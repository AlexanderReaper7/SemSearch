Set-StrictMode -Version Latest

# First.
function One([int]$X) {
    $y = $X + 1
    return $y * 2
}

<# Second. #>
function Two([int[]]$Items) {
    $total = 0
    foreach ($item in $Items) {
        $total += $item
    }
    return $total
}
