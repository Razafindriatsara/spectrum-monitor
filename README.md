# Spectrum Monitor

![Spektrum und Wasserfall](docs/screenshot.png)

*Oben das Live-Spektrum (gelb) mit Max-Hold (gestrichelt), unten der Wasserfall.
Max-Hold macht sichtbar, was im Momentbild fehlt: die beiden AIS-Kanäle in der
Mitte und den gesamten Bereich, den der Frequenzspringer rechts belegt.*

Echtzeit-Spektrumüberwachung und AIS-Empfang, komplett in Rust: IQ-Samples rein,
Spektrum, Wasserfall und Schiffe auf der Seekarte live im Browser. Das Frontend
ist Leptos (WebAssembly), Server und Frontend teilen sich ihre Datentypen.
Heute speist ein Signalsimulator das AIS-Band um 162 MHz mit Schiffsverkehr der
Kieler Förde, später kommt echter Empfang über einen RTL-SDR dazu.

## Starten

Einmalig die Werkzeuge für das WebAssembly-Frontend installieren:

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
```

Dann erst das Frontend bauen, danach den Server, der es einbettet:

```bash
cd frontend && trunk build --release && cd ..
cargo run --release
# dann http://127.0.0.1:8080 öffnen
```

Optionen: `--fft-size 4096` (feinere Auflösung), `--fps 30`,
`--detector avg|peak`, `--seed 7` (anderes Signalszenario), `--bind 0.0.0.0:8080`,
`--db ais.db` (SQLite-Datei, `:memory:` für flüchtig),
`--nmea-udp 127.0.0.1:10110` (NMEA-Ausgabe, z. B. für OpenCPN).

Beim Arbeiten am Frontend `trunk watch` in `frontend/` laufen lassen und den
Server im Debug-Build starten (`cargo run`). Er liest `frontend/dist/` dann bei
jeder Anfrage frisch von der Platte.

```bash
cargo test --release   # DSP, AIS-Kette von Bits bis Funksignal, Datenbank
```

## Architektur

```
                                   ┌─► SpectrumEstimator ──► broadcast ──► /ws ─────────────┐
IqSource (Trait)  ──► IQ-Block ────┤   Hann, FFT, Welch                                     ├──► Leptos-Frontend
 Simulator: Flotte,                └─► AisReceiver ──► AIS-Thread ──► broadcast ──► /ws/ais ─┤   Spektrum, Wasserfall,
 Träger, FM, Springer                  Kanal A und B    dekodieren,    SQLite ──► /api/…   ─┘   Seekarte, Protokoll
 später RTL-SDR                                         NMEA, UDP
```

Workspace mit drei Crates:

- **`spectrum-monitor`** (Wurzel): Server, Signalverarbeitung, AIS
  - `src/source.rs`: `IqSource`-Trait und Simulator. Er erzeugt einen Dauerträger,
    ein FM-Signal, einen Frequenzspringer und echte AIS-Aussendungen der
    simulierten Flotte, dazu Rauschen.
  - `src/dsp.rs`: Spektrumschätzung. Ein komplexer Ton mit Amplitude 1 ergibt
    0 dBFS. Der Detektor `peak` hält kurze Bursts sichtbar, `avg` glättet das Rauschen.
  - `src/ais/`: die AIS-Kette in beide Richtungen, siehe unten.
  - `src/store.rs`: SQLite mit allen Nachrichten (als NMEA), Positionsverläufen
    und dem zusammengeführten Stand jedes Schiffs.
  - `src/server.rs`: Axum mit WebSockets für Spektrum und AIS-Ereignisse, REST
    für Schiffe, Verläufe und Nachrichten, dazu das eingebettete Frontend.
  - `src/main.rs`: Erfassungsschleife in eigenem Thread. Sie verarbeitet pro Frame
    genau die Samples, die in 1/fps Sekunden anfallen, und gibt denselben Block an
    Spektrum und AIS-Empfänger. Dekodieren und Speichern laufen in einem zweiten
    Thread, damit Plattenzugriffe die Erfassung nicht aufhalten.
- **`api`**: gemeinsame Datentypen (`Vessel`, `AisEvent`, `SpectrumMeta` …) und
  die Beschreibung der Endpunkte. Server und Frontend serialisieren mit denselben Strukturen.
- **`frontend`**: Leptos im Browser, gebaut mit Trunk
  - `spectrum.rs`: Spektrum und Wasserfall auf Canvas, Marker, Max-Hold
  - `map.rs`: Seekarte aus OSM-Kacheln in Web-Mercator, Schiffe als SVG,
    Verlauf des ausgewählten Schiffs, Schiffsliste, Details, Ereignisprotokoll

## AIS

AIS-Sender an Bord melden Position, Kurs und Stammdaten auf 161,975 MHz (Kanal A)
und 162,025 MHz (Kanal B), GMSK-moduliert mit 9600 Bit/s. Das Projekt bildet die
Kette selbst ab, ohne fertige AIS-Bibliothek:

| Schicht | Senden (Simulator) | Empfangen |
|---|---|---|
| Nachricht (`message.rs`) | Typ 1–3 (Position) und 5 (Stammdaten) zu Bits | Bits zu Nachricht |
| Rahmen (`frame.rs`) | HDLC mit CRC-16, Bit-Stuffing, NRZI | Flag-Suche, Entstopfen, CRC-Prüfung |
| Physik (`gmsk.rs`, `demod.rs`) | GMSK mit Gaußfilter BT = 0,4 | Mischen, zweistufig dezimieren (2,4 MHz → 240 kHz → 48 kHz), FM-Diskriminator, Taktrückgewinnung |
| Ausgabe (`nmea.rs`) | | `!AIVDM`-Sätze, auch mehrteilig, per UDP |

Die Tests prüfen jede Schicht einzeln und die ganze Kette. Der Empfänger dekodiert
beide Kanäle gleichzeitig, verkraftet 500 Hz Frequenzfehler (3 ppm, typisch für
einen billigen RTL-SDR) und erzeugt aus reinem Rauschen keine Rahmen. Die
NMEA-Seite wird gegen einen echten Satz aus der AIVDM-Protokollbeschreibung geprüft.

Die simulierte Flotte (`fleet.rs`) besteht aus sechs erfundenen Schiffen: Fähren,
ein Frachter aus dem Nord-Ostsee-Kanal, ein Tanker vor Anker, ein Fischer und ein
Segler. Ihre Wegpunkte sind gegen die Wasserflächen der OSM-Karte geprüft. Nahe
Schiffe kommen stärker an als ferne, das ist im Spektrum zu sehen.

## Roadmap

1. **Spektrum und Wasserfall** mit Simulator ✔
2. **AIS-Dekodierung**: GMSK-Demodulation, NMEA, SQLite, Schiffe auf einer Karte ✔
3. **Frontend in Leptos** (Rust/WASM) statt JavaScript ✔
4. **RTL-SDR als Quelle** hinter demselben Trait, Mittenfrequenz im Browser umschaltbar
5. **Signaldetektion und Klassifikation**: CFAR-Detektor, Modulationsklassifikation
   per ONNX-Modell (Training in Python, Inferenz in Rust), Ereignisprotokoll
