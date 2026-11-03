// Motortest: GPIO 4 eine Sekunde an, zwei Sekunden aus, endlos. pio run -e motortest -t upload
#include <Arduino.h>

constexpr int MOTOR_PIN = 4;

void setup() {
  Serial.begin(115200);
  pinMode(MOTOR_PIN, OUTPUT);
  digitalWrite(MOTOR_PIN, LOW);
  Serial.println("Motortest: 1 s an, 2 s aus");
}

void loop() {
  digitalWrite(MOTOR_PIN, HIGH);
  Serial.println("an");
  delay(1000);
  digitalWrite(MOTOR_PIN, LOW);
  Serial.println("aus");
  delay(2000);
}
