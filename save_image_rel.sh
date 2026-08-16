espflash save-image \
  --chip esp32s3 \
  --flash-size 16mb \
  --partition-table ./partitions.csv \
  ./target/xtensa-esp32s3-espidf/release/app_esp32s3 \
  ./firmware.bin
