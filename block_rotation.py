faces = ["Front", "Back", "Up", "Down", "Right", "Left"]
axes = {"Front": "z", "Back": "z", "Up": "y", "Down": "y", "Right": "x", "Left": "x"}
output = "#[derive(Copy, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]\n"
output += "pub enum BlockRotation {\n"
cnt = 0
for first in faces:
    for second in faces:
        if axes[first] != axes[second]:
            output += "\t" + first + second + " = " + str(cnt) + ",\n"
            cnt += 1
output += "}\n"
output += "impl BlockRotation {\n"

output += "\t pub fn front_face(self) -> Face {\n"
output += "\t\tmatch self {\n"
for first in faces:
    for second in faces:
        if axes[first] != axes[second]:
            output += "\t\t\tBlockRotation::" + first + second + " => Face::" + first + ",\n"
output += "\t\t}\n"
output += "\t}\n"

output += "\t pub fn up_face(self) -> Face {\n"
output += "\t\tmatch self {\n"
for first in faces:
    for second in faces:
        if axes[first] != axes[second]:
            output += "\t\t\tBlockRotation::" + first + second + " => Face::" + second + ",\n"
output += "\t\t}\n"
output += "\t}\n"

output += "}\n"
print(output)