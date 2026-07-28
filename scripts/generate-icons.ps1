param(
    [string]$OutputDirectory = (Join-Path $PSScriptRoot "..\src\icons")
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

function New-RoundedRectanglePath {
    param(
        [float]$X,
        [float]$Y,
        [float]$Width,
        [float]$Height,
        [float]$Radius
    )

    $path = [System.Drawing.Drawing2D.GraphicsPath]::new()
    if ($Radius -le 0) {
        $path.AddRectangle([System.Drawing.RectangleF]::new($X, $Y, $Width, $Height))
        return $path
    }

    $diameter = [Math]::Min($Radius * 2, [Math]::Min($Width, $Height))
    $path.AddArc($X, $Y, $diameter, $diameter, 180, 90)
    $path.AddArc($X + $Width - $diameter, $Y, $diameter, $diameter, 270, 90)
    $path.AddArc($X + $Width - $diameter, $Y + $Height - $diameter, $diameter, $diameter, 0, 90)
    $path.AddArc($X, $Y + $Height - $diameter, $diameter, $diameter, 90, 90)
    $path.CloseFigure()
    return $path
}

function New-CodexGradientBrush {
    param([int]$Size)

    $brush = [System.Drawing.Drawing2D.LinearGradientBrush]::new(
        [System.Drawing.PointF]::new(0, 0),
        [System.Drawing.PointF]::new($Size, $Size),
        [System.Drawing.ColorTranslator]::FromHtml("#A99BFF"),
        [System.Drawing.ColorTranslator]::FromHtml("#302FE5")
    )
    $blend = [System.Drawing.Drawing2D.ColorBlend]::new(3)
    $blend.Positions = [single[]]@(0.0, 0.52, 1.0)
    $blend.Colors = [System.Drawing.Color[]]@(
        [System.Drawing.ColorTranslator]::FromHtml("#A99BFF"),
        [System.Drawing.ColorTranslator]::FromHtml("#617CF7"),
        [System.Drawing.ColorTranslator]::FromHtml("#302FE5")
    )
    $brush.InterpolationColors = $blend
    return $brush
}

function Add-UsageSegment {
    param(
        [System.Drawing.Graphics]$Graphics,
        [System.Drawing.Brush]$Gradient,
        [System.Drawing.Brush]$Track,
        [float]$X,
        [float]$Y,
        [float]$Width,
        [float]$Height,
        [float]$Radius,
        [double]$Fill
    )

    $path = New-RoundedRectanglePath $X $Y $Width $Height $Radius
    try {
        $Graphics.FillPath($Track, $path)
        if ($Fill -gt 0) {
            $state = $Graphics.Save()
            try {
                $fillWidth = [float]($Width * [Math]::Min(1.0, $Fill))
                $Graphics.SetClip(
                    [System.Drawing.RectangleF]::new($X, $Y, $fillWidth, $Height),
                    [System.Drawing.Drawing2D.CombineMode]::Intersect
                )
                $Graphics.FillPath($Gradient, $path)
            }
            finally {
                $Graphics.Restore($state)
            }
        }
    }
    finally {
        $path.Dispose()
    }
}

function New-CodexUsageIcon {
    param(
        [int]$Size,
        [string]$Path
    )

    $bitmap = [System.Drawing.Bitmap]::new($Size, $Size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $gradient = New-CodexGradientBrush $Size
    $track = [System.Drawing.SolidBrush]::new([System.Drawing.ColorTranslator]::FromHtml("#DAD9F6"))
    try {
        $graphics.Clear([System.Drawing.Color]::Transparent)
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality

        if ($Size -eq 16) {
            $margin = 0.5; $stroke = 1.0; $frameRadius = 1.0
            $columns = 2; $segmentX = 2.0; $segmentY = 2.0; $segmentWidth = 5.0
            $segmentHeight = 5.0; $gapX = 2.0; $gapY = 2.0; $segmentRadius = 1.0
            $firstRow = @(1.0, 0.4); $secondRow = @(1.0, 0.0)
        }
        elseif ($Size -eq 32) {
            $margin = 1.0; $stroke = 2.0; $frameRadius = 4.0
            $columns = 3; $segmentX = 3.0; $segmentY = 4.0; $segmentWidth = 8.0
            $segmentHeight = 11.0; $gapX = 1.0; $gapY = 2.0; $segmentRadius = 2.0
            $firstRow = @(1.0, 1.0, 0.5); $secondRow = @(1.0, 1.0, 0.0)
        }
        elseif ($Size -eq 48) {
            $margin = 2.0; $stroke = 4.0; $frameRadius = 6.0
            $columns = 5; $segmentX = 7.0; $segmentY = 10.0; $segmentWidth = 6.0
            $segmentHeight = 12.0; $gapX = 1.0; $gapY = 4.0; $segmentRadius = 2.0
            $firstRow = @(1.0, 1.0, 1.0, 0.5, 0.0); $secondRow = @(1.0, 1.0, 0.0, 0.0, 0.0)
        }
        else {
            $margin = 8.0; $stroke = 8.0; $frameRadius = 16.0
            $columns = 10; $segmentX = 15.0; $segmentY = 72.0; $segmentWidth = 19.0
            $segmentHeight = 48.0; $gapX = 4.0; $gapY = 16.0; $segmentRadius = 4.0
            $firstRow = @(1.0, 1.0, 1.0, 1.0, 1.0, 0.5, 0.0, 0.0, 0.0, 0.0)
            $secondRow = @(1.0, 1.0, 1.0, 0.25, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
        }

        $frameY = if ($Size -eq 256) { 36.0 } else { $margin }
        $frameHeight = if ($Size -eq 256) { 184.0 } else { $Size - 2.0 * $margin }
        $framePath = New-RoundedRectanglePath $margin $frameY ($Size - 2.0 * $margin) $frameHeight $frameRadius
        $pen = [System.Drawing.Pen]::new($gradient, $stroke)
        try {
            $graphics.DrawPath($pen, $framePath)
        }
        finally {
            $pen.Dispose()
            $framePath.Dispose()
        }

        for ($column = 0; $column -lt $columns; $column++) {
            $x = $segmentX + $column * ($segmentWidth + $gapX)
            Add-UsageSegment $graphics $gradient $track $x $segmentY $segmentWidth $segmentHeight $segmentRadius $firstRow[$column]
            Add-UsageSegment $graphics $gradient $track $x ($segmentY + $segmentHeight + $gapY) $segmentWidth $segmentHeight $segmentRadius $secondRow[$column]
        }

        $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    }
    finally {
        $track.Dispose()
        $gradient.Dispose()
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

$resolvedOutput = [System.IO.Path]::GetFullPath($OutputDirectory)
[System.IO.Directory]::CreateDirectory($resolvedOutput) | Out-Null
$sizes = @(16, 32, 48, 256)
foreach ($size in $sizes) {
    New-CodexUsageIcon $size (Join-Path $resolvedOutput "${size}x${size}.png")
}

function Get-IconDib {
    param(
        [int]$Size,
        [string]$PngPath
    )

    $bitmap = [System.Drawing.Bitmap]::new($PngPath)
    $stream = [System.IO.MemoryStream]::new()
    $writer = [System.IO.BinaryWriter]::new($stream)
    try {
        $maskStride = [int](([Math]::Floor(($Size + 31) / 32)) * 4)
        $writer.Write([uint32]40)
        $writer.Write([int32]$Size)
        $writer.Write([int32]($Size * 2))
        $writer.Write([uint16]1)
        $writer.Write([uint16]32)
        $writer.Write([uint32]0)
        $writer.Write([uint32]($Size * $Size * 4))
        $writer.Write([int32]0)
        $writer.Write([int32]0)
        $writer.Write([uint32]0)
        $writer.Write([uint32]0)

        for ($y = $Size - 1; $y -ge 0; $y--) {
            for ($x = 0; $x -lt $Size; $x++) {
                $pixel = $bitmap.GetPixel($x, $y)
                $writer.Write([byte]$pixel.B)
                $writer.Write([byte]$pixel.G)
                $writer.Write([byte]$pixel.R)
                $writer.Write([byte]$pixel.A)
            }
        }

        for ($y = $Size - 1; $y -ge 0; $y--) {
            $maskRow = [byte[]]::new($maskStride)
            for ($x = 0; $x -lt $Size; $x++) {
                if ($bitmap.GetPixel($x, $y).A -eq 0) {
                    $byteIndex = [int][Math]::Floor($x / 8)
                    $maskRow[$byteIndex] = $maskRow[$byteIndex] -bor (0x80 -shr ($x % 8))
                }
            }
            $writer.Write($maskRow)
        }

        $writer.Flush()
        return $stream.ToArray()
    }
    finally {
        $writer.Dispose()
        $stream.Dispose()
        $bitmap.Dispose()
    }
}

$iconData = [System.Collections.Generic.List[byte[]]]::new()
foreach ($size in $sizes) {
    [byte[]]$dib = Get-IconDib $size (Join-Path $resolvedOutput "${size}x${size}.png")
    [void]$iconData.Add($dib)
}
$iconPath = Join-Path $resolvedOutput "icon.ico"
$stream = [System.IO.File]::Open($iconPath, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write)
$writer = [System.IO.BinaryWriter]::new($stream)
try {
    $writer.Write([uint16]0)
    $writer.Write([uint16]1)
    $writer.Write([uint16]$sizes.Count)

    $offset = 6 + 16 * $sizes.Count
    for ($index = 0; $index -lt $sizes.Count; $index++) {
        $dimension = if ($sizes[$index] -eq 256) { 0 } else { $sizes[$index] }
        $writer.Write([byte]$dimension)
        $writer.Write([byte]$dimension)
        $writer.Write([byte]0)
        $writer.Write([byte]0)
        $writer.Write([uint16]1)
        $writer.Write([uint16]32)
        $writer.Write([uint32]$iconData[$index].Length)
        $writer.Write([uint32]$offset)
        $offset += $iconData[$index].Length
    }

    foreach ($bytes in $iconData) {
        $writer.Write($bytes)
    }
}
finally {
    $writer.Dispose()
    $stream.Dispose()
}
