# Writes a race that changes every second, to try the Live Timing input without a real timing system.
#   powershell -File simulate-timing.ps1 -Path .\timing-live.json
# Point the title's Live Timing source at the same file. Stop with Ctrl+C.
param(
    [string]$Path = (Join-Path $PSScriptRoot 'timing-live.json'),
    [int]$Cars = 16,
    [double]$Seconds = 1.0
)

$names = 'ALMEIDA', 'BARROS', 'CARVALHO', 'DIAS', 'ESTEVES', 'FARIAS', 'GOMES', 'HENRIQUE', 'IGLESIAS', 'JARDIM',
         'KRAUSE', 'LIMA', 'MOREIRA', 'NUNES', 'OLIVEIRA', 'PEREIRA', 'QUEIROZ', 'RAMOS', 'SOUZA', 'TEIXEIRA'
$random = [System.Random]::new(7)
$field = for ($i = 0; $i -lt $Cars; $i++) {
    [pscustomobject]@{
        no = [string](10 + $i * 7); fullname = $names[$i % $names.Count]
        class = if ($i % 3 -eq 2) { 'B' } else { 'A' }
        total_ms = 90000.0 + $i * 650; lap_ms = 91000.0 + $random.Next(0, 1500)
    }
}
$lap = 1
while ($true) {
    foreach ($car in $field) {
        $car.lap_ms = 90500.0 + $random.Next(0, 2500)
        $car.total_ms += $car.lap_ms / 60.0 + $random.Next(-3000, 3000) / 10.0   # enough noise for overtakes
    }
    $ranked = $field | Sort-Object total_ms
    $leader = $ranked[0].total_ms
    $drivers = for ($i = 0; $i -lt $ranked.Count; $i++) {
        $car = $ranked[$i]
        $inClass = ($ranked | Where-Object class -eq $car.class)
        $classLeader = $inClass[0].total_ms
        $fmt = {
            param($ms, $plus)
            $t = [TimeSpan]::FromMilliseconds([math]::Abs($ms))
            $s = '{0}:{1:00}.{2:000}' -f [int][math]::Floor($t.TotalMinutes), $t.Seconds, $t.Milliseconds
            if ($plus) { "+$s" } else { $s }
        }
        [ordered]@{
            no = $car.no; fullname = $car.fullname; class = $car.class
            position = $i + 1; positioninclass = [array]::IndexOf($inClass, $car) + 1
            lasttime = (& $fmt $car.lap_ms $false); total = (& $fmt $car.total_ms $false); total_ms = [int]$car.total_ms
            gap = if ($i -eq 0) { '' } else { (& $fmt ($car.total_ms - $leader) $true) }
            gap_in_class = if ($car.total_ms -eq $classLeader) { '' } else { (& $fmt ($car.total_ms - $classLeader) $true) }
        }
    }
    $document = [ordered]@{
        meta = [ordered]@{ eventname = 'SIMULATED GP'; runname = "RACE - LAP $lap"; laps = "$lap"; trackname = 'LIVEMIX CIRCUIT' }
        drivers = @($drivers)
    }
    $temporary = "$Path.tmp"
    $document | ConvertTo-Json -Depth 5 | Set-Content -Path $temporary -Encoding UTF8
    Move-Item -Path $temporary -Destination $Path -Force
    $lap++
    Start-Sleep -Seconds $Seconds
}
