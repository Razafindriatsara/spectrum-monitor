# Spectrum Monitor

![Spektrum und Wasserfall](docs/screenshot.png)

*Oben das Live-Spektrum (gelb) mit Max-Hold (gestrichelt), unten der Wasserfall.
Max-Hold macht sichtbar, was im Momentbild fehlt: die beiden AIS-Kanäle in der
Mitte und den gesamten Bereich, den der Frequenzspringer rechts belegt.*

Echtzeit-Spektrumüberwachung, komplett in Rust: IQ-Samples rein, Spektrum und
Wasserfall live im Browser, dazu AIS-Empfang mit Seekarte und eine
Signalüberwachung, die Sender per CFAR findet und ihre Modulation mit einem
selbst trainierten neuronalen Netz bestimmt. Das Frontend ist Leptos
(WebAssembly), Server und Frontend teilen sich ihre Datentypen, und auch das
Netz wird in Rust trainiert (candle) und läuft in Rust (tract).
Heute speist ein Signalsimulator das AIS-Band um 162 MHz mit Schiffsverkehr der
Kieler Förde und weiteren Sendern, später kommt echter Empfang über einen
RTL-SDR dazu.

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
`--nmea-udp 127.0.0.1:10110` (NMEA-Ausgabe, z. B. für OpenCPN),
`--cfar-db 10` (Detektionsschwelle über dem Rauschen).

Beim Arbeiten am Frontend `trunk watch` in `frontend/` laufen lassen und den
Server im Debug-Build starten (`cargo run`). Er liest `frontend/dist/` dann bei
jeder Anfrage frisch von der Platte.

```bash
cargo test --release   # DSP, AIS-Kette, Detektion, Klassifikation, Datenbank
```

Das Klassifikationsmodell liegt fertig in `models/modulation.onnx`. Neu trainieren:

```bash
cargo run --release --features train --bin train
```

## Architektur

```
                                   ┌─► SpectrumEstimator ──► broadcast ──────────► /ws ──────────┐
IqSource (Trait)  ──► IQ-Block ────┼─► AisReceiver ──► AIS-Thread ──► broadcast ──► /ws/ais ─────┤
 Simulator: Flotte,                │   Kanal A und B    dekodieren, NMEA, UDP                    ├──► Leptos-Frontend
 Träger, AM, FM, QPSK,             └─► Scanner-Thread ──► broadcast ───────────────► /ws/signals ┤   Spektrum, Wasserfall,
 Springer                              CFAR, Tracker,     SQLite ──────────────────► /api/… ─────┘   Signale, Seekarte
 später RTL-SDR                        CNN (tract)
```

Workspace mit drei Crates:

- **`spectrum-monitor`** (Wurzel): Bibliothek für die Signalverarbeitung, dazu
  der Server und der Trainer als Programme
  - `src/source.rs`: `IqSource`-Trait und Simulator. Er erzeugt einen Träger,
    AM, FM, QPSK, einen Frequenzspringer und echte AIS-Aussendungen der
    simulierten Flotte, dazu Rauschen. Die Modulatoren stehen in `src/signals.rs`.
  - `src/dsp.rs`: Spektrumschätzung. Ein komplexer Ton mit Amplitude 1 ergibt
    0 dBFS. Der Detektor `peak` hält kurze Bursts sichtbar, `avg` glättet das
    Rauschen. Dazu der `Downconverter`, der ein Schmalbandsignal auf 48 kHz
    mischt und dezimiert; AIS-Empfänger und Klassifikator nutzen ihn beide.
  - `src/ais/`: die AIS-Kette in beide Richtungen, siehe unten.
  - `src/detect.rs`, `src/classify.rs`: Signalüberwachung, siehe unten.
  - `src/bin/train/`: Trainer für den Klassifikator.
  - `src/store.rs`: SQLite mit allen Nachrichten (als NMEA), Positionsverläufen,
    dem zusammengeführten Stand jedes Schiffs und den Signalereignissen.
  - `src/server.rs`: Axum mit WebSockets für Spektrum, AIS und Signale, REST
    für Schiffe, Verläufe, Nachrichten und Signalereignisse, dazu das
    eingebettete Frontend.
  - `src/main.rs`: Erfassungsschleife in eigenem Thread. Sie verarbeitet pro Frame
    genau die Samples, die in 1/fps Sekunden anfallen, und gibt denselben Block an
    Spektrum, AIS-Empfänger und Scanner. Dekodieren, Klassifizieren und Speichern
    laufen in eigenen Threads, damit sie die Erfassung nicht aufhalten.
- **`api`**: gemeinsame Datentypen (`Vessel`, `AisEvent`, `SpectrumMeta` …) und
  die Beschreibung der Endpunkte. Server und Frontend serialisieren mit denselben Strukturen.
- **`frontend`**: Leptos im Browser, gebaut mit Trunk
  - `spectrum.rs`: Spektrum und Wasserfall auf Canvas, Marker, Max-Hold,
    erkannte Signale als farbige Bänder, Signalliste und Ereignisprotokoll
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

## Signalüberwachung

Der Scanner findet Sender im Spektrum, verfolgt sie über die Zeit und bestimmt
ihre Modulation: Träger, AM, FM, GMSK, QPSK oder Rauschen (Fehlalarm).

1. **CFAR** (`detect.rs`): Cell-Averaging-CFAR auf dem Spitzenspektrum. Für jedes
   Bin schätzen je 24 Trainingszellen links und rechts das Rauschen, 8
   Schutzzellen halten das Signal selbst heraus. Belegte Nachbarbins werden zu
   einem Signal mit Schwerpunktfrequenz und Breite zusammengefasst.
2. **Tracker**: ordnet Detektionen über Frames einander zu. Ein Signal gilt als
   bestätigt, wenn es 6 Frames am Stück da ist (Dauersender) oder in 5
   getrennten Bursts auf genau derselben Frequenz kommt (etwa AIS). Den
   Frequenzspringer filtert das heraus: Er bleibt nie lange genug, und dass er
   fünfmal zufällig dieselbe Frequenz auf ±600 Hz trifft, ist praktisch
   ausgeschlossen. Ein Test lässt ihn fünf Minuten laufen, ohne dass er ein
   einziges Mal bestätigt wird.
3. **Merkmale** (`classify.rs`): Signal auf 48 kHz mischen, das energiereichste
   Fenster von 1024 Samples nehmen (so landen auch Bursts darin), daraus drei
   Kanäle: Betrag, Momentanfrequenz und das Spektrum des Fensters. Alle drei
   sind unabhängig von Trägerphase und Pegel; I und Q fehlen bewusst, weil ein
   Träger genau bei 0 Hz darin ganz anders aussähe als einer knapp daneben.
4. **Klassifikation**: kleines 1D-CNN (vier Faltungen mit BatchNorm, Mittelwert-
   und Maximum-Pooling, zwei dichte Schichten, rund 45 000 Gewichte) als
   ONNX-Modell, ausgeführt mit tract. Die Wahrscheinlichkeiten mehrerer
   Klassifikationen werden pro Signal aufsummiert, das glättet Einzelfehler;
   gemeldet wird ein Signal erst nach fünf Stimmen. Hält das Netz ein Fenster
   für Rauschen, zählt das als Enthaltung. So verwässern Burst-Enden, die nur
   noch knapp in einen Frame ragen, die Klasse nicht, und ein Fehlalarm
   sammelt nie genug Stimmen, um gemeldet zu werden.

**Training in Rust** (`src/bin/train/`): Die Beispiele erzeugt der Trainer mit
denselben Modulatoren und derselben Merkmalsextraktion wie der Server, also
kann das Modell im Betrieb nichts anderes sehen als im Training. Zufällig sind
Rauschabstand (0–50 dB), Frequenzfehler, Phase und die Parameter jeder
Modulation. candle trainiert, und weil kein Rust-Framework ONNX exportiert,
schreibt der Trainer den Graphen selbst über die Protobuf-Typen von tract-onnx;
BatchNorm wird dabei in die Faltungsgewichte eingerechnet. Zum Schluss lädt er
das exportierte Modell mit tract und bricht ab, falls es von candle abweicht.
Auf 1800 getrennt erzeugten Validierungsbeispielen liegt die Genauigkeit bei
97,9 %.

## Roadmap

1. **Spektrum und Wasserfall** mit Simulator ✔
2. **AIS-Dekodierung**: GMSK-Demodulation, NMEA, SQLite, Schiffe auf einer Karte ✔
3. **Frontend in Leptos** (Rust/WASM) statt JavaScript ✔
4. **Signaldetektion und Klassifikation**: CFAR, Tracker, Modulationsklassifikation
   per ONNX-Modell (Training und Inferenz in Rust), Ereignisprotokoll ✔
5. **RTL-SDR als Quelle** hinter demselben Trait, Mittenfrequenz im Browser umschaltbar
