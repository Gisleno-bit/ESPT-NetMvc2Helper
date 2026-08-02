# MvC NetMon

Overlay de diagnóstico de netcode para **Marvel vs Capcom Fighting Collection**.

Mide en tiempo real la calidad de la conexión con tu rival y la muestra
encima del juego.

---

## Qué NO hace

Importante, porque define por qué esto es seguro:

- **No modifica el juego.** El `.exe` está protegido con Enigma Protector
  y ni se toca.
- **No inyecta nada** en el proceso del juego ni en el de Steam.
- **No altera el tráfico.** Solo observa. No retrasa, no duplica, no
  reordena paquetes. No hay forma de que esto dé ventaja a nadie.
- **No necesita Npcap** ni ningún driver. Usa el raw socket nativo de
  Windows (`SIO_RCVALL`).

Lo único que pide es **ejecutar como administrador**, que es requisito de
Windows para abrir un raw socket.

---

## Cómo se usa

1. Abre `mvc-netmon.exe` **como administrador** (clic derecho → Ejecutar
   como administrador).
2. Aparece un panel arriba a la izquierda. Es transparente a los clics:
   puedes pinchar a través de él sin problema.
3. Abre el juego. Debe estar en modo **ventana sin bordes**
   (`ScreenMode=BORDERLESS` en `config.ini`). En pantalla completa
   exclusiva el overlay no se ve.
4. Entra en un combate online. En cuanto detecta un flujo real, muestra
   los datos.

---

## Cómo se leen los números

### Jitter (el número grande)

**La métrica que importa.** Cuánto varía el tiempo entre paquete y
paquete. Un ping bajo con jitter alto se siente peor que un ping alto
estable, porque el rollback tiene que corregir a saltos impredecibles.

| Jitter | Qué significa |
|--------|---------------|
| < 3 ms | Excelente. Flujo regular como un metrónomo. |
| 3–7 ms | Bien. Ni lo notas. |
| 7–15 ms | Regular. Empieza a sentirse elástico. |
| > 15 ms | Malo. Aquí es donde la gente dice "el netcode es basura". |

### Gráfico de barras

Cada barra es un intervalo entre paquetes. Verde = regular,
amarillo = desviado, rojo = muy desviado. La línea azul es la media.

Un muro verde plano es una conexión sana. Un bosque de picos rojos es
tu problema, y ahora lo puedes *ver*.

### Ping ICMP

RTT hacia el rival. **Puede salir "sin respuesta" y no pasa nada**:
mucha gente tiene el ping bloqueado en el router. No es un fallo del
programa.

Nota honesta: este ping mide la ruta ICMP, no exactamente la del juego.
El tráfico de partida va cifrado y a 60 Hz continuo en ambos sentidos,
sin pares petición-respuesta, así que el RTT real **no se puede sacar de
la captura pasiva**. Este ICMP es la mejor aproximación disponible sin
tocar procesos protegidos.

### Intervalo

Milisegundos medios entre paquetes. A 60 Hz debería rondar los 16,7 ms.

### Pico y huecos

El pico es el mayor parón registrado. Los huecos son intervalos más de
3× la media: paquetes perdidos o retenidos. **Cualquier hueco durante un
combate se siente.**

---

## Log CSV

Si se activa la grabación, escribe `mvc-netmon-log.csv` junto al
ejecutable, una línea por segundo:

```
timestamp,ip,pps_in,pps_out,intervalo_ms,jitter_ms,pico_ms,huecos,rtt_ms
```

La idea a medio plazo es que varias personas compartan estos logs y se
pueda empezar a mapear qué rutas e ISPs dan mala conexión, que es
información que ahora mismo no tiene nadie.

---

## Compilar

No hace falta compilar en local. El workflow de GitHub Actions
(`.github/workflows/build.yml`) construye el `.exe` en los servidores de
GitHub; se descarga desde la pestaña Actions → último run → Artifacts.

En local, si alguna vez hiciera falta:

```
cargo build --release
```

---

## Estado

Versión 0.1. **Sin probar en hardware real todavía.**
