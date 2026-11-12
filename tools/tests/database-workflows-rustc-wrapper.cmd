@echo off
rem Cargo workspace wrapper only isolates this suite's workspace artifact hash.
rem Compiler and arguments are forwarded unchanged; registry dependencies are reused.
%*
