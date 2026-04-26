# Corvid Tutorial

## 1. Install

Unix:

```sh
sh install/install.sh
```

Windows PowerShell:

```powershell
./install/install.ps1
```

## 2. Check the machine

```sh
corvid doctor
```

## 3. Run a shipped demo

```sh
corvid tour --list
corvid tour --topic approve-gates
```

## 4. Audit a file before launch

```sh
corvid audit examples/refund_bot.cor
```

## 5. Build targets

```sh
corvid build examples/hello.cor
corvid build examples/hello.cor --target=native
corvid build examples/hello.cor --target=wasm
```
