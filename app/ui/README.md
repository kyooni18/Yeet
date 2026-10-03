# UI design specifications

The user will create the UI from scratch in OpenPencil. No layout is prescribed yet; ContentView.ui reserves the design entry point.

Once the user supplies or requests a design, describe meaningful regions with simple indentation-based layout notes. Agents read these files, inspect the implementation and update normal React Native components. These files are never loaded by the application. Do not create a parser, compiler, runtime renderer, or one file per primitive. Keep platform details in implementation code.
