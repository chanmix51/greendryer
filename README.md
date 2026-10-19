This project is my learning playground of Rust on ESP32. It has been coded using IA with the Cursor Editor. They are meant to be readable and tested. 

The subprojects are the following:

 * switch_state: a project with several workers sharing a state structure.
 * i2c_share: how to share an I2C bus between several workers (one 0.96" OLED SSD1306 screen and one SHT31 sensor).
 * greendryer: a more real life project to turn a fridge into a drying cabinet.
 * spi_display: use the ESP32's SPI bus at 20MHz to drive a 2" ST7789V display
 * rotary_spi: a simple game using interrupts with a KY-040 rotary encoder and the 2" ST7789V display 
